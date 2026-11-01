# B4 實機進入世界驗收證據與測試紀錄

**狀態**：`VERIFIED IN-WORLD (PASS)`  
**驗收日期**：2026-10-30 至 2026-10-31  
**驗收環境**：Windows 10 x64, Lineage Remastered x64 (2020-02-12 build), Java 8 L1RHD Server

---

## 1. 核心驗收結論

登入器 B4 路線（**中繼 2000/29467→3000 + 進程內八刀 `rhd_inproc.dll` + 中繼補發 S_KEY + opcode 199 明文帳密改寫**）已實機驗收通過：

1. **實機進世界與自由操作**：
   - 2026-10-30 21:20 成功進入遊戲世界，角色自由移動、開啟背包道具欄、開啟世界地圖、與 NPC 正常對話、開啟商店買賣。
   - 驗收截圖證明：[`reb4_world_ok.png`](reb4_world_ok.png)。
2. **雙端明文架構（ADR-005）成立**：
   - 服務端依 ADR-005 永久停發 `KeyPacket`（`verify-keypacket-stopped.ps1` 通過 GateB）。
   - 登入器中繼於連線建立後主動補發 13 位元組 S_KEY（opcode 15）喚醒客戶端，客戶端全域加密旗標保持為 0，上下行全程明文通訊。
3. **改進版觀測性（2026-10-31 驗收 PASS）**：
   - `rhd-launcher.exe` 啟動輸出 `BUILD_TAG` 標記與凍結風險快照（AscFileFilter 狀態、磁碟空間）。
   - 中繼 daemon 與補丁等待增加 10 秒心跳記錄。
   - 整合基於 `Login.local` 格式的 manifest 吃檔引擎（`updater.rs`，27/27 單元測試 PASS）。

---

## 2. 產物指紋與身份對齊

| 項目 | 路徑 / 名稱 | SHA-256 / 尺寸 | 備註 |
| :--- | :--- | :--- | :--- |
| **客戶端** | `bin64/Lin.bin` | `3490FF3F11D2E92DF4A99F72991054F6A2757AF737664FA1E05ED3F41A172AF0` | PE TimeDateStamp: `0x5E43D8D0` |
| **服務端核心** | `gs.jar` | `8C8707FEBB2D69E03383ED8869A54E9F2AB7F826E956F46EE02857FE8E3DB38C` | GateB 零 KeyPacket 引用 |
| **登入器主體** | `dist/rhd-launcher.exe` | `64f47728...` (16,067,072 bytes) | B4hb2 + file-sync 發行版 |
| **進程內補丁** | `dist/rhd_inproc.dll` | `e501ee0e...` (179,712 bytes) | 八刀交易式補丁 DLL |
| **實機進世界截圖** | `docs/evidence/reb4_world_ok.png` | `930,076 bytes` (已去敏裁切，僅保留遊戲視窗) | 進入世界實機畫面 |
| **清理稽核清單** | `docs/evidence/cleanup-manifest-20261030.tsv` | `2,901 bytes` | 歷史二進位清理前快照 |
| **清理執行結果** | `docs/evidence/cleanup-result-20261030.txt` | `2,487 bytes` | 釋放 8,547.6 MB 稽核紀錄 |

---

## 3. 實機封包序列證據鏈

中繼轉發日誌（`launcher.log`）與伺服端紀錄摘要：

```
21:16:48 [INPROC] Lin.bin 啟動（pid 29072），rhd_inproc.dll 注入
21:16:53 [INPROC] 八刀全部寫入覆核完成（rc=0x0，頁保護全部還原）
21:16:59 [RELAY]  客戶端連入中繼 [#1] (127.0.0.1:2000 -> 127.0.0.1:3000)
21:16:59 [RELAY]  補發 S_KEY（opcode 15，13 bytes: 0D 00 0F 42 90 E1 7A A4 29 70 53 00 00）
21:17:02 [CLIENT] C>S 161 bytes (可讀 Protobuf 明文握手)
21:17:02 [SERVER] S>C 90 bytes (opcode 240/821 版本資訊包)
21:17:09 [CLIENT] C>S 295 bytes (opcode 199 登入包)
21:17:09 [RELAY]  登入代理：opcode 199 已改寫為明文帳密包（17 bytes）
21:17:09 [SERVER] S>C 回傳 opcode 234 登入成功 + 系統公告
21:17:50 [UI]     進入角色選擇介面（6 角色格顯示正常）
21:17:56 [UI]     選角確認，客戶端開始批次載入
21:20:00 [WORLD]  順利進入遊戲世界，移動、背包、NPC 對話、地圖皆操作自如
```

服務端日誌同步佐證：
```
[INFO] 測試1 進入遊戲帳號==5566
[INFO] 目前在線玩家=1
```

---

## 4. 相關技術文件鏈接

- 八刀進程內補丁設計：[docs/B2-INPROC-LAUNCH.md](../B2-INPROC-LAUNCH.md)
- 原始黑曜石筆記：[docs/vault-notes/01-天R自製登入器與實裝實證/20261030-天R登入器B4中繼補發S_KEY與進入世界實證.md](../vault-notes/01-天R自製登入器與實裝實證/20261030-天R登入器B4中繼補發S_KEY與進入世界實證.md)
- 登入器歷史清理與考古索引：[docs/vault-notes/01-天R自製登入器與實裝實證/20261030-天R登入器檔案清理與考古索引.md](../vault-notes/01-天R自製登入器與實裝實證/20261030-天R登入器檔案清理與考古索引.md)
