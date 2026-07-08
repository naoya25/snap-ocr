# snap-ocr

macOS のメニューバーに常駐し、範囲選択したスクリーンショットを JAPAN AI CHAT API で OCR してクリップボードにコピーするツール。

## セットアップ

### 1. APIキーを設定する（いずれか一つ）

`.env` ファイル（推奨・一番手軽）:

```sh
cp .env.example .env
# .env を開いて JAPANAI_API_KEY と JAPANAI_USER_ID を記入
```

`.env` は起動時に自動読み込みされる（実行ディレクトリ → バイナリ隣接の順で探索）。`.gitignore` 済みなのでコミットされない。

環境変数:

```sh
export JAPANAI_API_KEY="sk-..."
```

または macOS Keychain に保存する（推奨・シェル履歴に残らない）:

```sh
security add-generic-password -s snap-ocr -a api-key -w "sk-..."
```

APIキーは `config.json` には一切書き込まれない。`.env`（環境変数として注入）→ 環境変数 → Keychain の順で解決する。

### 2. userId を設定する（どちらか一方）

userId はマイページのメールアドレス。

```sh
export JAPANAI_USER_ID="you@example.com"
```

環境変数を一度でも設定して起動すると、以後の利便性のために `~/Library/Application Support/snap-ocr/config.json` にも保存される（メールアドレスなので機密扱いしていない）。手動で `config.json` に書いてもよい:

```json
{ "user_id": "you@example.com", "model": "gemini-2.5-flash" }
```

### 3. 画面収録権限

`screencapture -i` はスクリーンショットのキャプチャに画面収録（Screen Recording）権限を必要とする場合がある。`cargo run` で起動する場合、**ターミナル（Terminal.app / iTerm など）自体**に「システム設定 → プライバシーとセキュリティ → 画面収録」の権限を付与すること。ビルド済みバイナリを直接実行する場合はそのバイナリに権限が要求される。

## 使い方

```sh
cargo run --release
```

起動するとメニューバーにアイコンが常駐する（Dock アイコンは出ない）。

- **⌥⌘8**（Cmd+Option+8）またはトレイメニューの「キャプチャしてOCR」で範囲選択 → OCR → クリップボードにコピー
- 選択中に Esc でキャンセルすると何も起きない
- OCR 中はトレイアイコンのタイトルが「⏳ OCR中…」になり、完了/失敗は macOS 通知で知らされる
- OCR 実行中に再度ホットキーを押しても無視される（多重実行防止）
- トレイメニューの「モデル」サブメニューで使用モデルを切り替え可能（即時 `config.json` に保存）
- 「モデル一覧を再取得」で `/v1/models` を再取得
- モデル一覧の取得に失敗した場合は既定リスト（`gemini-2.5-flash` / `gpt-5.4-nano` / `claude-4-5-haiku`）にフォールバックし、メニューに「(一覧取得失敗・既定リスト)」と表示される

## 既知の制限

- OCR のレイテンシはモデル依存で、数秒〜10秒を超えることがある（HTTP タイムアウトは120秒に設定）
- HEIC 等、JAPAN AI CHAT API 側が拒否する画像形式は非対応（本ツールは常に PNG でキャプチャするため通常は問題にならない）
- グローバルホットキー（⌥⌘8）は他アプリと衝突する場合、OS 側で無効化される可能性がある
- マルチディスプレイ環境や Retina 解像度でのキャプチャ挙動は `screencapture` コマンドの標準動作に準拠する
