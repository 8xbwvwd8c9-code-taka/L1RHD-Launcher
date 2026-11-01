# JJ 880 變身 / SPZ / 登入器知識報告

> Status: CANONICAL_RESEARCH_NOTE
> Date: 2026-09-13
> Scope: Lineage 3.81C historical evidence + 8.x/8.80 field evidence + current L880C runtime investigation
> Primary use: 給 AI/代理人快速建立正確心智模型，避免把 `list.spr`、`list.spz`、登入器變身 PAK、`spr_action` 混為同一層。

## 0. AI READ FIRST / HARD GUARDRAILS

1. **不要把 3.81C 的實測結果直接當成 8.80 的既定事實。** JJ-000106/107/108/109/110/112 的版本基線是 Lineage 3.81C + L1J-3.80c。
2. **不要宣稱 8.80 runtime 一定優先讀 `list.spz`。** 目前 880 專案尚未完成 runtime source precedence 的直接驗證。
3. **不要宣稱登入器自訂 PAK 一定存在或正在被目前 880 runtime 使用。** 目前只證明 ATU 880 Encoder GUI 具備「變身檔加密」能力；實際輸出格式與目前 Login/LinLogin 是否載入，仍待驗證。
4. **不要刪 `list.spz` 開頭的 `S`、不要猜 cipher、不要直接改 live Text.idx。** `S` 的語意未證實；高版本 SPZ 加密方式未確認。
5. **不要把 `gfxid` 當成實際 SPR prefix。** 變身編號可 alias 到另一個圖檔編號，例如 `#5641 64=240`。
6. **不要把 `spr_action` 當成客戶端圖像 resolver。** `spr_action` 是 server-side 動作/速度檢查資料；錯圖、紅叉、缺 animation mapping 的第一嫌疑通常在 morph mapping / resource path，不是 `spr_action`。
7. **版本資料不可盲目混用。** 880、8.5C、8.15、3.81C 的 loader/cipher/登入器行為可能不同。
8. **證據優先序：目前專案 runtime 實證 > 對應版本原始碼/工具 round-trip > JJ 實測文章 > 同版本論壇實測 > 異版本論壇經驗 > 推測。**

---

## 1. 核心心智模型

目前應把變身/怪物圖像鏈分成四層：

```text
Server DB / packet
    npc.gfxid / polyid
           |
           v
Runtime morph source selector
    |-----------------------------|
    |                             |
Client-native path          Launcher-custom path
Text.idx:list.spz           custom morph TXT -> Encoder -> morph PAK/blob
    |                             |
    |-----------------------------|
                  |
                  v
Morph block (#ID)
    - actual sprite/base mapping
    - action definitions
    - shadow/effect/alias/type
                  |
                  v
Sprite*.idx/pak actual SPR resources
                  |
                  v
Rendered NPC / polymorph

Separately:
Morph plaintext
    -> spr_action.exe/generator
    -> spr_action.sql
    -> server DB spr_action
    -> movement/attack/cast timing validation
```

**關鍵：** 圖像 mapping 與 server timing validation 是兩條不同的資料鏈。

---

## 2. JJ 3.81C 已直接實測的事實

### 2.1 Client-native path：`list.spz` 才是有效 morph source

來源：JJ-000106「客戶端和登入器與變身檔關係分析」

版本：
- Client: Lineage 3.81C
- Core: L1J-3.80c
- Launcher: L1J-3.80c

JJ 的操作結果：

- 關閉 Login.ini 的自訂變身檔，移除 `TW13081901.pak`。
- 單修改 `list.spr`，新增指定 morph block：**runtime 無效果**。
- 還原 `list.spr`，改 `list.spz` 並重新加密吃回：**runtime 生效**。

JJ 的結論（僅能視為 3.81C 版本事實）：

```text
client-native morph source = list.spz 主導
list.spr-only change        = ineffective
list.spz change             = effective
```

### 2.2 Launcher custom morph 啟用後，不 fallback 到 client list.spz

同一篇 JJ-000106 的後續實測：

- Login.ini 設定讀取自訂 `TW13081901.pak`。
- Client `list.spz` 內已存在的新增 morph，在 custom PAK 未包含時仍無效。
- 將相同 morph block 加進 `TW13081901.txt`、重新打包 `TW13081901.pak`，才恢復正常 animation。

因此在該 3.81C launcher：

```text
custom morph PAK enabled
        |
        +--> PAK 有定義 -> 使用 PAK
        +--> PAK 沒定義 -> 不 fallback client list.spz
```

**880 目前尚未直接證實相同行為，僅可當架構候選。**

### 2.3 每次修改 custom morph，PAK 與 Login.ini 應一起更新

JJ-000106 建議：

```text
TW13081901.txt edit
 -> regenerate TW13081901.pak
 -> update launcher setting / regenerate Login.ini
 -> deploy both
```

原因是只更新 PAK 時，JJ 曾遇過 runtime 未抓到正確變身編碼的情況。

---

## 3. `gfxid`、`polyid`、實際 SPR 的關係

來源：JJ-000107、JJ-000108。

### 3.1 定義

JJ 將：

- `gfxid` 視為圖像/圖形索引使用欄位。
- `polyid` 視為變身索引使用欄位。
- morph block 中 `#ID` 是變身編號；實際圖檔可與 `#ID` 相同，也可 alias 到另一個 sprite prefix。

例：

```text
#240 64 Death Knight
```

=> morph ID 240，未指定另一個圖檔，因此使用 `240-*.spr`。

```text
#5641 64=240 Death Knight
```

=> morph ID 5641，但實際 sprite/base 是 240。

因此：

```text
npc.gfxid / polyid
   != always actual SPR filename prefix
```

### 3.2 JJ-000108 在 3.81C DB 觀察到的 morph/gfx 欄位

`gfxid` 類：

```text
door_gfxs.gfxid
spawnlist_door.gfxid
mobskill.Gfxid
npc.gfxid
npc.agrogfxid1
npc.agrogfxid2
npc.transform_gfxid
trap.gfxId
weapon_skill.gfx_id
weapon_skill.gfx_id_target
```

`polyid` 類：

```text
armor_set.polyid
character_buff.poly_id
mobskill.PolyId
polymorphs.polyid
```

這份清單是 3.81C/L1J-3.80c schema 的觀察，不可假設所有 880 DB 欄位完全一致。

---

## 4. `spr_action` 的角色

來源：JJ-000109、JJ-000110。

### 4.1 產生鏈

```text
finished morph TXT
    -> spr_action.exe
    -> spr_action.sql
    -> DB spr_action
```

JJ-000109 指出一般模擬器利用 `spr_action` 判斷 client 移動、攻擊速度、施法速度是否異常。

### 4.2 重要欄位

```text
spr_id      morph ID / #ID
act_id      action ID
framecount  該動作使用的總時間/幀資料
framerate   播放速度資料
```

JJ-000110 觀察到 server 不是把所有 morph action 都放入 `spr_action`，主要保留與移動、攻擊、施法及部分物件動作有關的 action，例如：

```text
0  walk
1  attack
4/5   sword walk/attack
11/12 axe walk/attack
18    spell dir
19    spell nodir
20/21 bow walk/attack
24/25 spear walk/attack
40/41 staff walk/attack
46/47 dagger walk/attack
50/51 large sword walk/attack
54/55 double sword walk/attack
58/59 claw walk/attack
62/63 shuriken walk/attack
66/67 object/shop related
```

### 4.3 故障分類

```text
錯圖 / 紅叉 / 某 NPC 整體外觀錯
    -> first inspect morph source + mapping + actual SPR resources

只有特定 action 跳錯圖
    -> inspect that action's mapping/reference

動作卡頓 / 攻速施法速度不匹配 / server speed check 問題
    -> inspect spr_action + generated timing data
```

---

## 5. Morph plaintext 基礎規則

來源：JJ-000112。

JJ 3.81C 的變身資料存在三種 representation：

```text
list.spr
list.spz
TW13081901.txt   # launcher plaintext, name is version-dependent
```

### 5.1 第一行 count

JJ 觀察第一行第一個數字代表 morph entry count。

例如 count=11297，正常 ID 範圍為：

```text
#0 .. #11296
```

新增 `#20000` 時，第一個 count 應改為 `20001`，中間空缺 ID 需要補 dummy。

### 5.2 action block rules

JJ-000112 的版本觀察：

- 同一 morph ID 內，`action ID + action name` 不可出現內容不同的重複定義，否則可能造成 client crash。
- 原始檔已存在的 action 不建議刪除；刪除某 action 可能造成對應動作無法執行。

### 5.3 `list.spz` 與 launcher PAK 是不同打包路徑

JJ-000112：

```text
list.spz
  -> 不做 launcher PAK 打包
  -> 修改後需加密，再吃回 client Text container

TW13081901.txt
  -> launcher encoder
  -> TW13081901.pak
```

不要把 SPZ encryption 與 launcher morph PAK packaging 當成同一種格式。

---

## 6. SPZ / XML 舊版加解密證據

來源：JJ-000082。

舊工具：`Lineage-spz-xml.exe`

JJ 文章描述流程：

```text
encrypted XML/SPZ
   -> decrypt
   -> *.dec

modified plaintext
   -> rename suffix to *.e
   -> encrypt
   -> encrypted XML/SPZ
```

JJ 以 `polymorphList.xml` 與 `list.spz` 做過 round-trip，文章指出重新加密內容與原始加密內容一致。

### 6.1 可得出的安全結論

- 舊版 SPZ encryption 是可逆的。
- 該工具至少對 JJ 測試版本可 deterministic round-trip。
- `list.spz` plaintext grammar 與 morph/list 家族相容。

### 6.2 不可得出的結論

- **不能因此說 8.80 使用同一 cipher。**
- **不能因此說 8.80 可直接用舊 `Lineage-spz-xml.exe`。**
- 高版本論壇 field reports 曾出現 8.5C/8.x `list.spz` 對舊工具「格式異常」的情況，需視為版本差異風險。

---

## 7. 8.x / 8.80 field evidence（非 JJ 3.81C baseline）

### 7.1 新版 client 社群經驗

使用者提供的新版教學指出：

- 較新 client 修改 `list.spr` 可能已無效。
- 若變身編碼缺失，可能出現 SPR 不存在紅字、NPC 紅叉或錯誤圖像。
- 常見流程：抽 `Text.idx:list.spz` -> SPZ/XML 工具解密 -> 編輯 `.dec` -> 再加密 -> 吃回。
- 部分版本還需同步 `spr_action`。

這些是 field note，**不是 880 已驗證事實**。

### 7.2 `S` prefix

使用者提供論壇經驗提到部分 `list.spz` 解密內容前可能出現 `S`，有人移除後才生效，也有人移除後無法再正常加密。

880 local `list.spz` 的 binary prefix 也觀察到：

```text
S1748D ...
```

目前結論：

```text
S = UNKNOWN FORMAT/WRAPPER ARTIFACT
```

不要把它定義成 magic/version/checksum/seed，也不要直接刪除。

### 7.3 8.80 build IDs（論壇/登入器支援表線索）

研究線索常見：

```text
TW1810102501
TW1901142505
```

被社群標記為 8.80/8.8C 相關 build。這些 build ID 應作為後續搜尋 `Lin.bin` / `Login.bin` / `list.spz` / morph loader / encoder 的精準關鍵字，但仍須以實際 880 client binary metadata 驗證。

---

## 8. ATU 880 Encoder：目前直接證據

目前 880 套件內存在：

```text
LinLauncher/
  Encoder.exe
  Encoder.ini
  LinLauncher.exe
  LauncherDll.dll
  LinLogin.bin
  880變身檔.txt
```

`Encoder.exe` version metadata：880，Company=ATU。

GUI 已人工確認有：

```text
變身檔加密
[選擇文件]
[加密文件]
[ ] 變身檔瘦身（沒事不要選）
```

因此可直接證明：

```text
ATU 880 Encoder has a morph-file encryption/packaging function.
```

但目前仍未知：

```text
- output filename / extension
- output binary format
- whether it is a launcher PAK/blob or client list.spz
- whether current 880 Login/LinLogin loads that output
- whether “瘦身” only normalizes plaintext or alters semantic actions
```

**不要再從 `Encoder.ini` 只有 ServerList 欄位就推論 Encoder 只能處理 ServerList；GUI 已否定該推論。**

---

## 9. Current L880C runtime evidence

### 9.1 Live client morph files

目前從 880 client `Text.idx` 抽出：

```text
list.spr
size   = 6,798,758
SHA256 = AE71564BEAD6F96EE2EE52331D60F1CC54A30D5F9D1AE214AF4A662E2FB81ED9

list.spz
size   = 6,798,759
SHA256 = FF724FA04437A274683C130EDB2BAAEB30A437B00670D9EC3CF0B4D2D4081E1A
```

Pre-patch `list.spz` 與 live `list.spz` SHA256 完全相同。

因此已確定：

```text
之前 patch 只更新 list.spr
list.spz 完全沒有被更新
```

但這 **不等於** 已證明 880 runtime 一定讀 `list.spz`。

### 9.2 `list.spr` plaintext mapping 對問題 NPC 看起來合理

已觀察 mapping：

```text
15606 -> actual sprite 1037
15607 -> actual sprite 1039
15608 -> actual sprite 1047
15648 -> actual sprite 4060
15694 -> actual sprite 6302
```

對應 live DB 問題 NPC 包含：

```text
56502 巨蟻       gfx 15606
56503 巨大兵蟻   gfx 15607
56507 巨大白螞蟻 gfx 15694
56508 毒蠍       gfx 15608
56509 沙毒蠍     gfx 15648
```

實際 SPR 資源也已找到；因此「單純 SPR 檔案不存在」已不是主要嫌疑。

### 9.3 Runtime symptom

多個不同沙漠 NPC 在 runtime 顯示成相同/近似的長袍 humanoid，而 server-side DB/packet gfx trace 與 live `list.spr` mapping 皆合理。

目前較合理的 root-cause family：

```text
1. runtime 使用另一份 morph source（stale list.spz / launcher custom morph）
2. Login/LinLogin/Lin.bin 有 morph mapping/validation/override
3. 版本不相容造成 action/base mapping drift
4. actual SPR missing（目前核心問題 ID 證據較弱）
5. server gfx packet wrong（核心五隻已有 trace，優先度降低）
```

---

## 10. Current hypothesis ranking

### H1 — runtime source mismatch

最值得優先驗證。

```text
live list.spr correct
but runtime may consume:
- unchanged/stale list.spz
OR
- launcher custom morph blob/PAK
```

### H2 — launcher/client binary mapping or validation

`LinLogin.bin` / `Lin.bin` / launcher DLL 可能存在版本特定 morph table、ID limit、action override 或 loader logic。

已有 8.8C field note 顯示某些編號會被登入器硬寫 action；可作為 positive-control 研究方向，但目前不可推廣到所有 gfx ID。

### H3 — high-version SPZ cipher/version drift

舊 `Lineage-spz-xml` 可能不支援 8.80。若確定 runtime 走 client `list.spz`，才值得進一步研究 8.80 cipher。

### H4 — `spr_action` mismatch

主要對速度/動作 timing 有影響；不是目前「整體外觀錯成 humanoid」的第一嫌疑。

---

## 11. Preferred research / fix sequence

不要先逆 cipher。先解 runtime source。

```text
STEP 1  Prove runtime morph source
        A. client Text.idx:list.spz
        B. launcher custom morph PAK/blob
        C. another embedded/overridden source

STEP 2A if launcher-custom path
        Determine ATU Encoder input/output
        -> copy-only sandbox test
        -> hash before/after
        -> inspect output
        -> verify Login/LinLogin reference

STEP 2B if native SPZ path
        Identify 8.80 build and cipher generation
        -> acquire compatible source/tool if possible
        -> original SPZ decrypt
        -> NO-CHANGE encrypt
        -> compare SHA256 / plaintext correctness
        -> only after round-trip gate, edit COPY

STEP 3  Verify one known positive morph ID
        -> minimal one-ID change
        -> test copy/client backup

STEP 4  Re-generate spr_action only when morph actions/timing changed
```

### Required round-trip gate for any candidate SPZ encoder

```text
original list.spz
 -> decrypt
 -> NO EDIT
 -> encrypt
 -> compare with original
```

Best signal：byte-identical SHA256；若 tool 使用 randomized encryption，ciphertext 不同不一定代表失敗，但必須至少證明 decrypt plaintext 正確且 client 可驗證。

---

## 12. Fault classification cheat sheet for AI

```text
SYMPTOM: red X / missing sprite
CHECK: morph entry -> actual SPR prefix -> Sprite container existence

SYMPTOM: whole NPC is consistently the wrong creature/person
CHECK: active morph source / #ID mapping / alias / launcher override

SYMPTOM: only attack/walk/cast frame jumps into wrong character
CHECK: that specific action mapping + referenced SPR frames

SYMPTOM: animation is static/no breathing
CHECK: morph action block missing or active source has only base image mapping

SYMPTOM: movement/attack/cast feels too fast/slow or server flags abnormal speed
CHECK: spr_action row generation / framecount / framerate / core timing check

SYMPTOM: changes to list.spr do nothing
CHECK: active source may be list.spz or launcher custom morph; do not assume list.spr runtime use
```

---

## 13. Source map

Primary JJ articles:

- JJ-000082 — https://morosedog.gitlab.io/private-lineage-20220215-private-lineage-82/
- JJ-000103 — https://morosedog.gitlab.io/private-lineage-20220508-private-lineage-103/
- JJ-000106 — https://morosedog.gitlab.io/private-lineage-20220511-private-lineage-106/
- JJ-000107 — https://morosedog.gitlab.io/private-lineage-20220512-private-lineage-107/
- JJ-000108 — https://morosedog.gitlab.io/private-lineage-20220513-private-lineage-108/
- JJ-000109 — https://morosedog.gitlab.io/private-lineage-20220514-private-lineage-109/
- JJ-000110 — https://morosedog.gitlab.io/private-lineage-20220515-private-lineage-110/
- JJ-000112 — https://morosedog.gitlab.io/private-lineage-20220517-private-lineage-112/

Additional evidence classes used in this report:

- USER_PROVIDED_FIELD_NOTES: Lineage45 / LinHelper / LineageCN excerpts supplied during 2026-09-13 investigation.
- LOCAL_L880C_EVIDENCE: DB queries, packet trace, Text.idx extraction, hashes, list mapping, SPR inventory, screenshots and ATU Encoder GUI observed during current project.
- UPLOADED_DB_GUIDES: `DB聖經大全配.rar`, `商業版專用DB解說.rar`; distilled separately in `research/JJ_DB_REFERENCE_INGEST_20260913.md`.

---

## 14. Canonical one-paragraph summary for other AI

**For the current 880 investigation, do not equate DB `gfxid`, morph `#ID`, actual SPR prefix, client `list.spz`, launcher morph PAK, and server `spr_action`. JJ 3.81C testing proves client-native morphs were controlled by encrypted `list.spz`, while a configured launcher custom morph PAK fully replaced that source without fallback; however this precedence is not yet proven for 8.80. Current L880C evidence shows `list.spr` was updated but `list.spz` stayed byte-identical, while DB/packet gfx and plaintext mappings for the problematic NPCs look correct and their SPR assets exist. Therefore first prove which morph source the 880 runtime actually consumes. If it is launcher-custom, reverse only the ATU 880 Encoder input/output path; if it is native SPZ, identify a compatible 8.80 cipher/tool and require a no-edit decrypt/encrypt round-trip before touching live Text.idx. `spr_action` is a separate server timing-validation table and should not be treated as the primary resolver for wrong NPC graphics.**
