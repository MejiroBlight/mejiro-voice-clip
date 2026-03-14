# mejiro-voice-clip

音声・動画ファイルからセリフ単位のクリップを切り出してWAVエクスポートするデスクトップアプリケーションです。

## スタック

| レイヤー | 技術 |
|---|---|
| フレームワーク | [Tauri v2](https://tauri.app/) + [SvelteKit](https://kit.svelte.dev/) + TypeScript |
| 波形表示 | [WaveSurfer.js](https://wavesurfer.js.org/) (RegionsPlugin / ZoomPlugin / TimelinePlugin) |
| 音声デコード | [Symphonia](https://github.com/pdeljanov/Symphonia) + symphonia-adapter-fdk-aac |
| WAV書き出し | [hound](https://github.com/ruuda/hound) |

## 主な機能

### ファイル読み込み・波形表示

- MP4 / MP3 / WAV を開くと動画プレビューと波形を同時表示
- Rust バックエンドが60秒チャンク単位でピークを計算しイベント送信 → 波形がリアルタイムに描画される
- 動画ストリームは Tauri カスタムプロトコル (`http://stream.localhost/`) 経由でシークをサポート

### リージョン編集

- **Temp リージョン** (青): Q/E キーで開始・終了点を打ち、波形上でリサイズ可能
- **タグ** でリージョンに色を付けて視覚的に分類 (タグダイアログで追加・編集・削除)
- リージョン一覧で並び替え・編集・削除・個別再生
- リージョンの編集モード (`Add` / `Edit`) をセレクトボックスで切り替え

### WAV エクスポート

- チェックしたリージョンをまとめてエクスポート
- ファイル名フォーマットを4種類から選択
  - `index_tag_name` / `tag_name` / `start_tag_name` / `name`
- Rust 側でリージョン開始時刻にシークしてから必要な範囲だけデコード → ファイル全体をメモリに展開しない

### キーボードショートカット

全操作をキーボードショートカットで実行できます。
設定はダイアログ (Keyboard アイコン) から変更でき、AppData に保存・起動時に読み込まれます。
キー競合がある場合はダイアログを閉じることができず、競合行が赤くハイライトされます。

| アクション | デフォルトキー |
|---|---|
| 再生 / 一時停止 | `Space` |
| -0.5秒 | `←` |
| +0.5秒 | `→` |
| スタートマーカーへジャンプ | `W` |
| Temp 開始点をセット | `Q` |
| Temp 終了点をセット | `E` |
| Temp リージョンを再生 | `S` |
| Temp リージョンをリセット | `R` |
| リージョンを追加 / 編集確定 | `F` |
| タグ選択にフォーカス | `A` |
| リージョン名入力にフォーカス | `D` |

## 開発環境のセットアップ

推奨: [VS Code](https://code.visualstudio.com/) + [Svelte 拡張](https://marketplace.visualstudio.com/items?itemName=svelte.svelte-vscode) + [Tauri 拡張](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

```bash
# 開発サーバー起動
npm run tauri dev

# リリースビルド
npm run tauri build
```
