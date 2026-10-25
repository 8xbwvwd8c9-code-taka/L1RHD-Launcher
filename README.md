# RHDLauncher

Rust 桌面啟動器，提供伺服器設定、TCP relay、更新腳本整合與遊戲啟動管理。背景使用純色繪製，不依賴客戶端美術素材。

## 建置

需求：Windows、Rust stable 與 Cargo。

cargo build --release

首次執行會在程式旁建立設定檔。請依 servers.example.ini 設定伺服器位址、連接埠及遊戲目錄；帳號密碼由遊戲客戶端處理。不要把實際伺服器設定或憑證提交到 Git。

## 功能與限制

- Windows 桌面介面與伺服器清單。
- 本機 TCP relay 轉送客戶端連線。
- 可選更新腳本與更新後套用流程。
- hosts 受控區段管理及遊戲程序啟動。
- 舊版 direct-launch 程式碼保留作歷史用途，預設關閉；它會操作遊戲程序記憶體，不屬於一般使用流程。

## 專案說明

AI 開發指南見 AI_GUIDE.md。執行檔、客戶端資產、個人設定、日誌及分析工具不包含在此原始碼倉庫。