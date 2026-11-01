# Legacy Launcher Morph Artifact Study (3.7/3.8/7.6-family evidence)

> Status: DERIVED_RESEARCH_NOTE
> Date: 2026-09-13
> Scope: user-supplied launcher/encoder artifacts + public field reports
> Purpose: establish how launcher morph PAK, client-native list.spz, spr_action SQL, and morph plaintext relate without mixing versions.

## 0. AI READ FIRST

1. This note contains **artifact-level evidence** from supplied files. Treat it as stronger than generic forum memory, but do not promote a 3.7/7.6 behavior into an 8.80 fact without matching-version validation.
2. `launcher morph PAK` and client `Text.idx:list.spz` are separate paths. Do not call them the same format.
3. The new plaintext `change.txt` evidence weakens the earlier hypothesis that a leading `S` must belong only to an encrypted SPZ wrapper. `S` semantics remain UNKNOWN.
4. Never overwrite live 880 resources from these older artifacts. Use copied test material and round-trip gates.

---

## 1. Supplied artifact inventory

| Artifact | Size | SHA-256 | Key observation |
|---|---:|---|---|
| `change.txt` | 5,908,131 | `C434CA6EEEF0376D0ACCAC865FB521BA618F44F514A8310862DF4877DC8A31E5` | plaintext morph dataset; header `S20840 0 41210` |
| `TW13032701.txt` | 5,541,855 | `3125D0C62DBCDD3CA4436336B266231469B707F8176183CFE0E8DF6DF585A149` | plaintext morph dataset; header `17754 0 41210` |
| `TW13032701.pak` | 5,541,855 | `34B41864A48C2CBF03CF6B38F306E0431BB45FE9E05A432B29B35147DF457CB7` | encrypted/high-entropy launcher morph PAK; **same byte length as TXT pair** |
| `TW13081901.pak` | 7,456,819 | `DEED55E2563FFB823D5BD852BF6E8BABFCBDB12107C4E0B43D9D9AACF028ECE7` | encrypted/high-entropy launcher morph PAK; plaintext counterpart not supplied |
| `spr_action.sql` | 4,274,508 | `75D5F576A0124D5193B5EBFEBC7474261D4D1266E98F55F3751C3E1D3AF7E4D9` | 70,163 `spr_action` INSERT rows |
| `TW13032701.bin` | 2,582,376 | `049E0579C193A666665EEC0C10C4E28E8AA2184F1C0D26C1767E77212E6BEDA4` | PE executable / client-era binary artifact |
| `Login.ini` | 169 | `3F8A5F0F29B5E458AEA7FC9F1290F60566778E93BC1B640C2AAE0165B37413EC` | encrypted `ServerData` launcher connection record |
| `Login.cfg` | 408 | `C5100F5DFE7C96B151084A04FCD4AF5007D39F8B9EFD5B652EBF51D864BDAFD2` | launcher UI/update config; no plaintext morph filename observed |
| `list.txt` | 1,061 | `3691CA944851FBF2B07F6E0B642A4DC2005CB2A72648D38CD2DE839D877E6D8B` | `[update]` + encrypted `ServerData0`; server list role |
| `pack.properties` | 92 | `258C00D0F50E8A452E9CE09E20D0CB9E7B69A90D4A273E28A684D70ACA7372F7` | server packet-auth/encryption configuration; exact key values intentionally omitted here |

The supplied 7.6C encoder screenshot visibly separates:

```text
List generation / server settings
    [x] built-in morph file -> change.pak

Patch-file processing
    Morph file: [dropdown] [encrypt]
    XML encrypt: [dropdown] [encrypt]
```

This is strong GUI evidence that the launcher ecosystem treated **morph-file encryption** and **XML encryption** as separate operations, and that a launcher could explicitly select a custom morph PAK (`change.pak`).

---

## 2. `TW13032701.txt` vs `change.txt`

### 2.1 Header / ID-space structure

`TW13032701.txt`:

```text
17754 0 41210
```

Observed block IDs are continuous from `#0000` through `#17753` (17,754 blocks).

`change.txt`:

```text
S20840 0 41210
```

Observed:

- block-marker occurrences: 20,842
- unique block IDs: 20,837
- minimum/maximum: 0 / 20,839
- missing IDs: 15,030; 16,179; 20,602
- duplicate IDs: 20,701..20,705 (each occurs twice)

Therefore `change.txt` is not a clean 1:1 continuous canonical table despite its `S20840` header. Do not blindly normalize it without understanding its producer/consumer rules.

### 2.2 Dummy-to-filled migration

Literal `dummy` blocks:

```text
TW13032701.txt : 2,185
change.txt      :   184
```

Within IDs 15,000..17,753 alone, at least 1,085 IDs that are literal `dummy` in `TW13032701.txt` are populated with non-dummy definitions in `change.txt`.

This directly demonstrates a real historical failure mode:

```text
older launcher morph table
    -> high ID remains dummy
    -> resource may exist but morph mapping/action definition is absent

newer/extended morph table
    -> same ID becomes fully defined
    -> launcher can resolve/use the sprite actions
```

This is highly relevant to the current 880 investigation, but only as a structural analogy until the 880 runtime source is proven.

---

## 3. High-value target IDs in supplied artifacts

### 3.1 IDs currently relevant to the 880 visual investigation

In `TW13032701.txt` the following are literal dummy entries:

```text
#15606 dummy
#15607 dummy
#15608 dummy
#15648 dummy
#15669 dummy
#15694 dummy
#15986 dummy
```

`#16040` has only a minimal action definition.

In `change.txt`, the same IDs have real morph definitions, including:

```text
#15606 40=1037  giant ant
#15607 40=1039  giant ant soldier
#15608 48=1047  scorpion
#15648 48=4060  scorpion purple
#15669 56=4771  mutation lizard man
#15694 48=6302  giant acid ant
```

These mappings match the independently observed 880 `list.spr` mappings for the same IDs.

This is strong evidence that the 15xxx mappings are not inventions from the current 880 project; they existed in older extended launcher morph datasets too.

### 3.2 Known launcher-hardcoded / special-case probe IDs

`change.txt` contains full action blocks for:

```text
#5727
#5730
#5733
#5736
```

The user previously supplied an 8.8C note warning that these IDs were explicitly handled by a launcher and should be remapped to new IDs to avoid action errors. `change.txt` now provides concrete historical definitions for those probe IDs.

### 3.3 8.8C sample block overlap

`change.txt` also contains rich blocks for:

```text
#15986
#16040
```

Their action structure matches the same IDs seen in the user's separately obtained 8.8C polymorph-definition samples.

However `change.txt` has:

```text
#17486 1
102 0
```

while the separately obtained 8.8C package supplied a non-dummy `#17486` example. This is direct evidence of **version/content drift between morph datasets** even when IDs overlap.

---

## 4. Launcher PAK transform evidence

A known plaintext/ciphertext pair is available:

```text
TW13032701.txt : 5,541,855 bytes
TW13032701.pak : 5,541,855 bytes
```

The PAK has near-random byte distribution (measured entropy about 7.97 bits/byte), while plaintext entropy is much lower.

Important conclusions:

1. The historical transform used for this pair is **size-preserving**.
2. The output is not a normal padded block-cipher container (file length is not aligned to 16 bytes).
3. No compression-size change is present in this pair.
4. A simple fixed repeating XOR key was not demonstrated by preliminary comparison; do not claim the cipher yet.

Do **not** equate this historical PAK with the modern open-source `L1J-3.8-launcher` PAK implementation, whose README describes AES-128 + HMAC-SHA256 + zlib. That project is a modern reimplementation and its container format is not proven identical to the historical encoder used here.

### Next cryptographic gate

Because a full known plaintext/ciphertext pair exists, the correct research order is:

```text
1. identify the exact historical encoder/decrypter implementation if source exists
2. otherwise characterize transform using copied pair only
3. require decrypt(TW13032701.pak) == exact TW13032701.txt
4. require encrypt(TW13032701.txt) == exact known PAK if deterministic
5. only then test TW13081901.pak / change.pak
```

---

## 5. `spr_action.sql` correlation

Artifact statistics:

```text
INSERT rows     = 70,163
max spr_id      = 17,751
unique spr_id   = 8,709
```

For current 15xxx targets, rows are absent when the corresponding old morph source is dummy. Example:

```text
15606 -> no spr_action rows
15607 -> no rows
15608 -> no rows
15648 -> no rows
15669 -> no rows
15694 -> no rows
15986 -> no rows
16040 -> one minimal action row
```

This strongly links this SQL artifact to the older `TW13032701.txt`-era morph source rather than the richer `change.txt` dataset.

It reinforces the model:

```text
launcher/client morph source controls graphical/action mapping
             |
             +--> spr_action generator derives server timing/action rows
```

If the morph plaintext changes substantially, regenerating `spr_action.sql` may be required for server timing consistency, even though `spr_action` itself is not the client sprite resolver.

---

## 6. Config-file role separation

### `list.txt`

Structure:

```text
[update]
ver=...
url=...
[list]
ServerData0=<encoded blob>
```

Role: launcher update/server-list data. Not morph plaintext.

### `Login.ini`

Structure:

```text
[Server1]
ServerName=...
ServerData=<encoded blob>
```

Role: per-server launcher connection/config record. The morph filename may be embedded in the encrypted blob for some launcher generations, so absence of a plaintext filename does not prove morph PAK is disabled.

### `pack.properties`

Contains packet authentication/RSA settings used by server-side packet encryption/authentication. Keep this conceptually separate from morph PAK encryption.

---

## 7. Public field evidence that corroborates the artifact model

Public forum reports independently describe:

- if a morph ID is absent/dummy in the launcher morph TXT, copy/add the block, run the encoder's morph function, then deploy the resulting `TW13032701.pak`/`TW13081901.pak` and `Login.ini`;
- some launchers can leave the custom morph-file option unchecked and use client-native `list.spz` instead;
- 7.6-family launcher deployment examples explicitly list `change.pak`, `LauncherDll.dll`, `LinHelperZ`, `LinLauncher`, `LinLogin.bin` together;
- a public thread specifically distributes a decrypter supporting APlus `TW13032701.pak` and `TW13081901.pak` (`PakDecrypt.rar`), confirming these launcher morph PAKs are reversible in practice.

These are field reports, not format specifications. Their value is architectural corroboration, not proof of the 8.80 cipher.

---

## 8. Correction to previous `S` hypothesis

Previous working hypothesis:

```text
S in 880 encrypted list.spz might be generated by encryption/wrapper layer
```

New artifact evidence:

```text
change.txt begins with plaintext:
S20840 0 41210
```

Therefore the stronger safe conclusion is now:

```text
leading S semantics = UNKNOWN
```

It may be part of a particular morph plaintext dialect/version marker, an input marker consumed by the encoder/launcher, or another format convention. **Do not classify it as encryption-only and do not strip it without a matching-version round-trip test.**

This is an important correction for future AI agents.

---

## 9. Updated decision tree for 880

```text
A. Prove current 880 runtime source
   |
   +-- custom launcher morph enabled?
   |       |
   |       +-- YES -> locate exact PAK/blob selected by Login/LinLogin
   |       |          -> decrypt/compare to 880 plaintext
   |       |
   |       +-- NO  -> client-native list.spz path becomes primary
   |
B. If launcher PAK path:
   obtain/generate known 880 TXT<->PAK pair
   -> characterize exact 880 transform

C. If client SPZ path:
   identify matching 8.80 SPZ/XML cipher
   -> require no-edit decrypt/encrypt round-trip

D. After morph mapping is corrected:
   regenerate/check spr_action only for timing/action consistency
```

---

## 10. Highest-value missing artifact

The single most useful next artifact is **the actual `change.pak` paired with this supplied `change.txt`**, or the exact encoder that generated it.

That would give a second plaintext/ciphertext pair and answer whether `change.pak` uses the same transform family as `TW13032701.pak`.

For 880 specifically, the equivalent gold-standard pair is:

```text
880 morph plaintext
<->
actual PAK/blob loaded by ATU 880 LinLauncher/LinLogin
```

Until that pair or runtime load proof exists, do not replace the live 880 `list.spz` or assume launcher-PAK precedence.
