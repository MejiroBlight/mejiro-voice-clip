# Implementation Status (実装状況)

このドキュメントは、現在のアプリケーションの実装状況を分かりやすくまとめています。

## ✅ 現在の完成済み（実装済み）

- **プロジェクト構成**：SvelteKit + Tauri の標準テンプレートとして初期化済み。
- **フロントエンド（Svelte）**
  - ルートページ (`src/routes/+page.svelte`) にメディアファイルを選択して波形を表示する UI を実装。
    - WaveSurfer.js を使い、再生/一時停止、時間移動、波形表示、タイムライン、リージョン作成・管理が可能。
    - 選択したファイルは `@tauri-apps/plugin-fs` の `readFile` で読み込み、Blob URL として WaveSurfer に渡して再生する。
  - タグ管理機能（タグの追加・編集・色指定）を実装し、リージョンに色を割り当てる仕組みを含む。
  - `<dialog>` を用いたタグ管理モーダルを実装し、クリックで閉じる挙動などの UI 動作を実装。
  - 一部で `@tauri-apps/api/core` の `invoke` も利用可能な状態（Rust 側コマンド呼び出し対応）を残したままになっている。
- **バックエンド（Rust）**
  - `src-tauri/src/lib.rs` に `greet(name: &str) -> String` コマンドが実装されている。
  - `src-tauri/src/lib.rs` に `extract_audio_from_video(input_path: String) -> String` コマンドを追加し、Symphonia で MP4 から音声トラックを抽出して WAV を出力する機能を試験的に実装。
  - AAC が `malformed stream: aac: invalid data` になる場合に備えて、`symphonia-adapter-fdk-aac` を導入し、AAC 時は FDK AAC デコーダを使うようフォールバック実装を追加。
  - `tauri-plugin-dialog` を使ってファイルダイアログで MP4 を選択できるようにした。
  - `src-tauri/src/main.rs` から `mejiro_voice_clip_lib::run()` を実行し、Tauri アプリを起動する構成。
- **ビルド/実行**
  - `npm run tauri dev` で開発環境起動可能。

## ❌ 未実装/未完了（今後要対応）

- 音声クリップ（voice clip）に関する機能は未実装。
  - 音声録音・再生・ファイル保存などの機能はまだ存在しない。
  - UI/UX の要件（録音ボタン、タイムライン、ファイル管理など）は定義されていない。
- データ管理・設定（例：アプリ設定、ユーザー設定、保存先パスなど）は未実装。
- テスト（ユニットテスト/統合テスト）は未実装。

---

## 次のステップ（例）

1. まずアプリで達成したい最初のユースケースを定義する（例：録音してファイルとして保存する）。
2. 必要な UI 要素とメニュー構成を設計する。
3. Tauri の API（`@tauri-apps/api`）と Rust 側の処理を組み合わせて実装する。

---

> **注意**：今後のコード変更は `CODEGEN_RULES.md` に従って、実装状況ドキュメントを必ず更新してください。
