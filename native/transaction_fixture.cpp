// This fixture only modifies its own private allocation and is not linked into the launcher or DLL.
#include <windows.h>
#include <array>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>

using Bytes = std::array<unsigned char, 4>;
static constexpr Bytes kOriginal0 = {0x10, 0x20, 0x30, 0x40};
static constexpr Bytes kOriginal1 = {0x50, 0x60, 0x70, 0x80};
static constexpr Bytes kReplacement0 = {0x11, 0x21, 0x31, 0x41};
static constexpr Bytes kReplacement1 = {0x51, 0x61, 0x71, 0x81};
static constexpr std::uint32_t kFixtureVersion = 0x12345678;

struct Patch {
    std::size_t offset;
    Bytes original;
    Bytes replacement;
};

enum class Outcome {
    committed, no_sync, wrong_version, invalid_range, overlap, wrong_preimage,
    safely_aborted, unsafe_stop
};

struct Context {
    unsigned char* memory;
    std::size_t page_size;
    std::array<Patch, 2> patches;
    std::string fault;
    bool injected = false;
    unsigned protect_calls = 0;
    unsigned writes = 0;
    unsigned flushes = 0;
};

static bool page_has_protection(const Context& c, std::size_t page, DWORD protection) {
    MEMORY_BASIC_INFORMATION info = {};
    return VirtualQuery(c.memory + page * c.page_size, &info, sizeof(info)) == sizeof(info) &&
           info.AllocationBase == c.memory && info.State == MEM_COMMIT &&
           info.Type == MEM_PRIVATE && info.Protect == protection;
}

static bool page_is_executable_read(const Context& c, std::size_t page) {
    return page_has_protection(c, page, PAGE_EXECUTE_READ);
}

static bool set_page(Context& c, std::size_t page, DWORD protection, DWORD& previous) {
    ++c.protect_calls;
    return VirtualProtect(c.memory + page * c.page_size, c.page_size, protection, &previous) != 0;
}

static bool flush(Context& c, bool rollback) {
    ++c.flushes;
    const bool actual = FlushInstructionCache(GetCurrentProcess(), c.memory, c.page_size * 2) != 0;
    if ((c.fault == "flush-failure" && !rollback && !c.injected) ||
        (c.fault == "rollback-failure" && (!c.injected || rollback))) {
        c.injected = true;
        return false;
    }
    return actual;
}

// Independent executable specification for the fixture outcomes.
static Outcome expected_for(const std::string& scenario) {
    if (scenario == "success") return Outcome::committed;
    if (scenario == "no-sync") return Outcome::no_sync;
    if (scenario == "version-mismatch") return Outcome::wrong_version;
    if (scenario == "preimage-mismatch") return Outcome::wrong_preimage;
    if (scenario == "range-invalid") return Outcome::invalid_range;
    if (scenario == "overlap") return Outcome::overlap;
    if (scenario == "rollback-failure" || scenario == "protect-race") return Outcome::unsafe_stop;
    return Outcome::safely_aborted;
}

static Outcome rollback(Context& c) {
    bool complete = true;
    std::array<bool, 2> writable = {};
    for (std::size_t page = 0; page < writable.size(); ++page) {
        DWORD previous = 0;
        writable[page] = set_page(c, page, PAGE_READWRITE, previous);
        if (writable[page] && previous != PAGE_EXECUTE_READ && previous != PAGE_READWRITE) {
            DWORD ignored = 0;
            set_page(c, page, previous, ignored);
            writable[page] = false;
        }
        complete = writable[page] && complete;
    }
    for (const auto& patch : c.patches) {
        if (!writable[patch.offset / c.page_size]) {
            complete = false;
            continue;
        }
        std::memcpy(c.memory + patch.offset, patch.original.data(), patch.original.size());
        complete = std::memcmp(c.memory + patch.offset, patch.original.data(), patch.original.size()) == 0 && complete;
    }
    complete = flush(c, true) && complete;
    for (std::size_t page = 0; page < writable.size(); ++page) {
        DWORD previous = 0;
        if (writable[page]) complete = set_page(c, page, PAGE_EXECUTE_READ, previous) && complete;
        complete = page_is_executable_read(c, page) && complete;
    }
    return complete ? Outcome::safely_aborted : Outcome::unsafe_stop;
}

static Outcome apply(Context& c, bool fixture_quiescent, std::uint32_t version) {
    if (!fixture_quiescent) return Outcome::no_sync;
    if (version != kFixtureVersion) return Outcome::wrong_version;

    const auto size = c.page_size * 2;
    for (const auto& patch : c.patches) {
        if (patch.offset > size - patch.original.size() ||
            patch.offset / c.page_size != (patch.offset + patch.original.size() - 1) / c.page_size) {
            return Outcome::invalid_range;
        }
    }
    const auto& a = c.patches[0];
    const auto& b = c.patches[1];
    if (a.offset < b.offset + b.original.size() && b.offset < a.offset + a.original.size()) {
        return Outcome::overlap;
    }
    if (a.offset / c.page_size == b.offset / c.page_size) return Outcome::invalid_range;
    for (const auto& patch : c.patches) {
        if (!page_is_executable_read(c, patch.offset / c.page_size)) return Outcome::invalid_range;
        if (std::memcmp(c.memory + patch.offset, patch.original.data(), patch.original.size()) != 0) {
            return Outcome::wrong_preimage;
        }
    }

    std::array<bool, 2> writable = {};
    std::array<DWORD, 2> protections = {};
    for (std::size_t page = 0; page < writable.size(); ++page) {
        if (c.fault == "protect-race" && page == 1 && !c.injected) {
            DWORD previous = 0;
            const bool changed = set_page(c, page, PAGE_READONLY, previous);
            if (!changed || previous != PAGE_EXECUTE_READ) {
                DWORD ignored = 0;
                if (changed) set_page(c, page, previous, ignored);
                for (std::size_t i = 0; i < page; ++i) {
                    if (writable[i]) set_page(c, i, protections[i], ignored);
                }
                return Outcome::unsafe_stop;
            }
            c.injected = true;
        }
        if (c.fault == "protect-failure" && page == 1 && !c.injected) {
            c.injected = true;
        } else {
            writable[page] = set_page(c, page, PAGE_READWRITE, protections[page]);
        }
        if (!writable[page] || protections[page] != PAGE_EXECUTE_READ) {
            const bool unexpected = writable[page] && protections[page] != PAGE_EXECUTE_READ;
            bool restored = true;
            for (std::size_t i = 0; i <= page; ++i) {
                if (writable[i]) {
                    DWORD ignored = 0;
                    restored = set_page(c, i, protections[i], ignored) && restored;
                    restored = page_has_protection(c, i, protections[i]) && restored;
                } else {
                    restored = page_is_executable_read(c, i) && restored;
                }
            }
            return restored && !unexpected ? Outcome::safely_aborted : Outcome::unsafe_stop;
        }
    }

    for (std::size_t i = 0; i < c.patches.size(); ++i) {
        const auto& patch = c.patches[i];
        ++c.writes;
        if (c.fault == "partial-write" && i == 1 && !c.injected) {
            c.injected = true;
            std::memcpy(c.memory + patch.offset, patch.replacement.data(), 2);
            return rollback(c);
        }
        std::memcpy(c.memory + patch.offset, patch.replacement.data(), patch.replacement.size());
        if (std::memcmp(c.memory + patch.offset, patch.replacement.data(), patch.replacement.size()) != 0) {
            return rollback(c);
        }
    }
    if (!flush(c, false)) return rollback(c);
    for (std::size_t page = 0; page < writable.size(); ++page) {
        DWORD previous = 0;
        if (c.fault == "restore-failure" && page == 1 && !c.injected) {
            c.injected = true;
            return rollback(c);
        }
        if (!set_page(c, page, PAGE_EXECUTE_READ, previous) || previous != PAGE_READWRITE) {
            return rollback(c);
        }
    }
    return page_is_executable_read(c, 0) && page_is_executable_read(c, 1)
        ? Outcome::committed : rollback(c);
}

int main(int argc, char** argv) {
    if (argc != 2) return 2;
    const std::string scenario = argv[1];
    const std::array<const char*, 12> cases = {"success", "no-sync", "version-mismatch",
        "preimage-mismatch", "range-invalid", "overlap", "protect-failure", "protect-race", "partial-write",
        "flush-failure", "restore-failure", "rollback-failure"};
    bool known = false;
    for (const char* name : cases) known = known || scenario == name;
    if (!known) return 2;

    SYSTEM_INFO info = {};
    GetSystemInfo(&info);
    const std::size_t page = info.dwPageSize;
    if (page < 128) return 3;
    auto* memory = static_cast<unsigned char*>(VirtualAlloc(nullptr, page * 2,
        MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE));
    if (!memory) return 3;
    std::memcpy(memory + 64, kOriginal0.data(), kOriginal0.size());
    std::memcpy(memory + page + 64, kOriginal1.data(), kOriginal1.size());
    DWORD previous = 0;
    if (!VirtualProtect(memory, page * 2, PAGE_EXECUTE_READ, &previous) || previous != PAGE_READWRITE) {
        VirtualFree(memory, 0, MEM_RELEASE);
        return 3;
    }

    Context context{memory, page, {{{64, kOriginal0, kReplacement0},
                                   {page + 64, kOriginal1, kReplacement1}}}, scenario};
    bool quiescent = scenario != "no-sync";
    std::uint32_t version = scenario == "version-mismatch" ? 0 : kFixtureVersion;
    if (scenario == "preimage-mismatch") context.patches[1].original[0] ^= 0xff;
    if (scenario == "range-invalid") context.patches[1].offset = page * 2 - 2;
    if (scenario == "overlap") context.patches[1].offset = 65;
    const Outcome result = apply(context, quiescent, version);

    const Outcome expected = expected_for(scenario);
    const auto& expected0 = result == Outcome::committed ? kReplacement0 : kOriginal0;
    const auto& expected1 = result == Outcome::committed ? kReplacement1 : kOriginal1;
    const DWORD final_protection1 = scenario == "protect-race" ? PAGE_READONLY : PAGE_EXECUTE_READ;
    bool ok = result == expected && page_is_executable_read(context, 0) &&
        page_has_protection(context, 1, final_protection1) &&
        std::memcmp(memory + 64, expected0.data(), expected0.size()) == 0 &&
        std::memcmp(memory + page + 64, expected1.data(), expected1.size()) == 0;
    if (result == Outcome::no_sync || result == Outcome::wrong_version ||
        result == Outcome::invalid_range || result == Outcome::overlap || result == Outcome::wrong_preimage) {
        ok = ok && context.protect_calls == 0 && context.writes == 0 && context.flushes == 0;
    }
    if (scenario == "protect-failure" || scenario == "protect-race" || scenario == "partial-write" || scenario == "flush-failure" ||
        scenario == "restore-failure" || scenario == "rollback-failure") ok = ok && context.injected;
    if (scenario == "success") ok = ok && context.writes == 2 && context.flushes == 1;
    if (scenario == "protect-failure" || scenario == "protect-race") {
        ok = ok && context.writes == 0 && context.flushes == 0;
    }
    if (scenario == "partial-write") ok = ok && context.writes == 2 && context.flushes == 1;
    if (scenario == "flush-failure" || scenario == "restore-failure" || scenario == "rollback-failure") {
        ok = ok && context.writes == 2 && context.flushes == 2;
    }
    if (!VirtualFree(memory, 0, MEM_RELEASE)) ok = false;
    if (!ok) std::fprintf(stderr, "fixture failed: %s outcome=%d protect=%u writes=%u flush=%u\n",
        scenario.c_str(), static_cast<int>(result), context.protect_calls, context.writes, context.flushes);
    return ok ? 0 : 1;
}
