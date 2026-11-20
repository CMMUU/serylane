<!-- docs -->
# 入門與平台支援

先挑選符合架構的正式安裝檔，再使用自己的訂閱或 Mihomo 設定。此頁介紹主要功能、平台限制和發行記錄。

## 快速入門 {#start}

安裝後新增並選用訂閱，或從設定檔匯入本機 YAML。啟用系統代理後，在「連線」確認實際請求。詳細步驟請參考[使用指南](site:guide)。

## 目前功能 {#current-capabilities}

可管理訂閱、選擇節點、查看連線、日誌和診斷。自選節點和 AI 代理二選一；[網域路由](site:routing)預設關閉。程式代理和新增 Codex 接入目前停用。

## TUN 與平台限制 {#tun-compatibility}

macOS 需 Developer ID 簽署、Apple 公證的完整安裝檔，以及輔助程式許可。Windows TUN 仍屬實驗性，需要管理員權限。Linux 不提供 TUN；系統代理使用 GNOME gsettings。

## 下載與架構 {#download-center}

從[官方下載區](site:home#download)選擇 Windows、macOS 或 Linux 的 x64／ARM64 安裝檔。Mac 的 M 系列選 Apple 晶片。版本與架構以[下載中心](https://downloads.cmmuu.com/projects/serylane)正式記錄為準。

## 歷史發行記錄 {#history}

早期功能與驗證記錄保留在[簡體中文存檔](/zh/docs/#history)或[英文存檔](/docs/#history)。過往記錄並不代表目前下載的版本。

<!-- routing -->
# 依網域指定出口

僅將已進入代理核心、且符合名單的 AI 網域連線交給指定出口。網域路由預設關閉。

## 工具和自訂網域 {#node-metadata}

可選 OpenAI、Claude 和 Gemini 的預設範圍，或新增自訂網域。`example.com` 符合根網域；`*.example.com` 只符合子網域。取消工具不會刪除自訂項目。

## 儲存後再套用 {#save-apply}

明確選擇本訂閱的出口群組，儲存草稿，再確認啟用或套用變更。未符合名單的流量保留原規則。如果更換訂閱後找不到出口，請重選，不會改走直連。

## 頁首與路由頁 {#global-controls}

上方路由按鈕和路由頁共用開關，路由頁也可手動刷新狀態。兩個控制都不會啟動核心或切換系統代理與 TUN。

## 驗證是否就緒 {#enablement}

確認核心在規則模式，並檢查規則和流量入口。只顯示草稿已儲存不代表路由生效。用一條新連線核對網域和出口。

## 停止代理時的狀態 {#proxy-routing-state}

核心停止時，已啟用的路由會暫停但保留設定。再次手動啟動後才重新核對，不會只因路由開啟而自動啟動。

## 節點選擇與穩定性 {#stability}

AI 代理與自選節點互斥；前者接受 1–10 個合格節點，只有一個時無備援。切換只影響後續連線，不會續接已中斷的請求。

## 限制 {#validation-boundaries}

路由不會解密 HTTPS、不依加密路徑分流，也不修改 Codex 設定。新增 Codex 接入、程式代理及舊增強穩定策略停用。詳細操作見[使用指南](site:guide#ai-tools)。

<!-- verification -->
# 驗證下載與更新

明確核對正式發布來源、原始檔名、架構、SHA-256 和簽章。這些檢查和系統權限是不同的事。

## 下載來源 {#download-origin}

從[官方下載區](site:home#download)或[下載中心](https://downloads.cmmuu.com/projects/serylane)選擇相符的正式安裝包，GitHub 原始碼連結不是安裝檔的備援來源。

## 比對 SHA-256 {#checksums}

從同一正式版取得安裝檔及 `SHA256SUMS.txt`，核對完全相同的檔名和雜湊。不符就不要安裝。以下命令中的檔名要用實際下載檔替換。

- Windows：`Get-FileHash -LiteralPath ./installer.exe -Algorithm SHA256`
- macOS：`shasum -a 256 ./installer.dmg`
- Linux：`sha256sum ./installer.AppImage`

## 更新簽章 {#update-signatures}

應用程式的更新機制會驗證專案簽章。人工核對時使用同一版本提供的簽章、清單和公鑰，不要以不明來源的公鑰替代。

## 作業系統簽署 {#system-signing}

專案更新簽章不是 Windows Authenticode、Apple Developer ID 或公證的證據。以該版本的發行記錄為準。

## macOS TUN 授權 {#macos-tun}

使用簽署且公證的完整包。舊輔助服務若報 `EX_CONFIG`，新版需確認舊登記已退出且新版握手通過。依提示在登入項目與延伸功能核准，回到程式繼續，再檢查是否真正接管流量。

## 更新下載中斷 {#update-downloads}

順序為 GitHub 直連、GitHub 加速、經 Serylane 代理、下載中心。切換來源仍須核對同版本、同架構、大小、SHA-256 和簽章，驗證失敗時不繼續安裝。

## 安裝失敗 {#update-installation}

保存失敗階段與已遮蔽的錯誤。先檢查空間與權限；舊版內部更新一直失敗時，改用正式完整包覆蓋同一位置，保留設定。

## 歷史核驗記錄 {#verification-history}

需查閱早期版本細節時，請參考[簡體中文核驗記錄](/zh/docs/verification#verification-history)或[英文核驗記錄](/docs/verification#verification-history)。

<!-- privacy -->
# 隱私與資料範圍

官網、下載中心及桌面用戶端各自處理不同資料。瀏覽官網不會開啟代理或自動連結 Codex。

## 官網行為 {#website-data}

提供靜態內容和公開更新資訊。官網無帳號、分析或廣告追蹤器，不要求上傳訂閱、權杖或設定，不呼叫本機代理。

## 本機偏好 {#theme-preference}

瀏覽器只儲存 `serylane-theme` 指定外觀。語言由網址決定，無語言 Cookie 或本機語言偏好；外觀也不會送到伺服器。

## 主機請求 {#hosting}

提供網頁與保護服務所需的基本存取資訊，例如 IP、時間、網址及瀏覽器資訊，會由主機與傳遞服務處理。

## 下載與外部連結 {#downloads}

下載路由核對公開版本資料和對應安裝包，不會轉發訪客 Cookie 或授權標頭。下載中心處理送出的檔案請求；外部站點各自負責其資料。見[驗證說明](site:verification)。

## 桌面用戶端 {#desktop-client}

訂閱、設定與網域分流是桌面功能。網域規則在本機處理，預設關閉，不解密 TLS。訂閱網址可能有存取憑證，分享前要遮蔽。

## 回報問題 {#reporting}

使用 [GitHub 問題回報](https://github.com/CMMUU/serylane/issues)，不要公開訂閱網址、密碼、權杖或原始設定。安全漏洞請先查看專案是否有私人通報管道。
