# Serylane 使用手冊

從挑選安裝檔到匯入訂閱、選擇代理模式、設定 AI 網域分流、更新及排除常見問題。

## 下載與安裝
<!-- anchor: install -->

從[官方網站](https://serylane.cmmuu.com/zh-hant/#download)或[下載中心](https://downloads.cmmuu.com/projects/serylane)取得正式版。請自備節點、訂閱及 AI 服務帳號。

- Windows：Intel 或 AMD 64 位元電腦選 x64；Windows on ARM 選 ARM64。
- macOS：M 系列選 Apple 晶片，Intel Mac 選 Intel。開啟 DMG，將 Serylane 拖至「應用程式」。需要 macOS 13 或更新版本。
- Linux：選擇符合架構的 AppImage，授予執行權限後執行。

## 快速開始
<!-- anchor: quick-start -->

1. 在「訂閱 → 新增訂閱」填入名稱和 Mihomo / Clash Meta 訂閱網址，驗證後新增。如果使用本機 YAML，從「設定檔」開啟並驗證再啟用。
2. 確認目標設定已選用。新增訂閱不會自動取代已選用的設定，選用也不會啟動代理。
3. 先試上方的「系統代理」。啟動後開啟熟悉的網站，從「連線」確認請求已進入核心，必要時查看「診斷」。

## 選擇代理模式
<!-- anchor: proxy-modes -->

- **系統代理**適用於遵循系統代理的程式。Linux 使用 GNOME gsettings，其他桌面可手動設定本機連接埠。
- **TUN**適合不讀取系統代理的程式。macOS 必須使用經簽署及公證的完整包，依提示到「系統設定 → 一般 → 登入項目與延伸功能」允許輔助程式。Windows TUN 為實驗功能且需管理員權限；Linux 未實作。
- **本機連接埠**：在需要代理的程式填入 `127.0.0.1` 和 Serylane 的混合連接埠，按需要選擇 HTTP 或 SOCKS5。請勿填寫控制連接埠。

輔助程式就緒不等於接管流量；請查驗執行狀態和實際連線。規則、全域及直連決定流量進入核心後如何處理。

## 管理訂閱
<!-- anchor: subscriptions -->

可刷新單張卡片或全部訂閱；用量和設定獨立刷新，失敗時保留可用設定。從「更多」或滑鼠右鍵編輯，修改網址後再刷新。若匯入或刷新失敗，確認使用服務商提供的 Mihomo / Clash Meta 訂閱網址，而非帳號網頁，並查看下載路徑及出錯階段。

## 自選節點與 AI 代理
<!-- anchor: nodes -->

這兩種模式互斥。自選節點可測速並從訂閱群組選擇出口；AI 代理接受 1–10 個合格節點，只有一個時沒有備援。手動選擇後按「套用節點」，待顯示已套用；想重新自動選擇時按「恢復自動」。節點變更只影響後續連線。

## AI 工具、網域路由與 Codex
<!-- anchor: ai-tools -->

網域路由預設關閉，只處理已進入核心的規則模式流量。選擇工具或填寫網域和出口，儲存草稿，再啟用或套用。`example.com` 是根網域，`*.example.com` 只符合子網域。請參閱[網域路由](https://serylane.cmmuu.com/zh-hant/docs/routing)。

在支援此選項的 macOS 或 Windows Codex CLI 中，可用 `codex --enable respect_system_proxy` 讀取已存在的系統代理，只作用於本次啟動；它不啟用 PAC，也不為 PAC 增加故障切換。參閱 [OpenAI 說明](https://learn.chatgpt.com/docs/enterprise/managed-configuration#use-the-system-proxy-for-native-client-requests)，以實際完成一個請求來確認。

## 檢查與安裝更新
<!-- anchor: updates -->

在「設定 → 軟體更新」檢查版本與實際來源。依序嘗試 GitHub 直連、GitHub 加速、經運作中的 Serylane 代理及下載中心，核對版本、架構及簽章後才能安裝。舊版無法內部更新時，用同架構正式完整包覆蓋安裝，保留設定。查看[驗證說明](https://serylane.cmmuu.com/zh-hant/docs/verification)。

## 疑難排解
<!-- anchor: troubleshooting -->

- 顯示執行中但程式未經代理：核對系統代理或 TUN 是否真的接管，再看「連線」。
- macOS TUN 等待授權或提示重新關聯：先查登入項目權限；執行重新關聯前停止代理。
- 切回系統代理顯示連接埠使用中：查看技術詳情。剛停止 TUN 時，讓舊連線釋放再試。模式回復不代表代理啟動。
- AI 測試回傳 HTTP 401：只表示端點可達，還需驗證登入和真實模型請求。

回報問題請提供版本、平台、網路模式、發生時間和已遮蔽機密的錯誤摘要，不附訂閱網址或權杖。
