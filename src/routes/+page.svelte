<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open } from '@tauri-apps/plugin-dialog';
  import { readTextFile, writeTextFile, mkdir, BaseDirectory } from '@tauri-apps/plugin-fs';
  import WaveSurfer from 'wavesurfer.js';
  import RegionsPlugin from 'wavesurfer.js/dist/plugins/regions.esm.js';
  import ZoomPlugin from 'wavesurfer.js/dist/plugins/zoom.esm.js';
  import TimelinePlugin from 'wavesurfer.js/dist/plugins/timeline.esm.js'
  import { onMount, untrack } from "svelte";
  import {
    FileDown, Play, Pause, StepBack, StepForward, SkipBack,
    ArrowLeftToLine, ArrowRightToLine, Disc, Trash2, Tag, Folder, Keyboard
  } from "@lucide/svelte";

  type Tag = {
    name: string;
    color: string;
  };

  let fileName = $state("No file selected");
  let inputPath: string | null = $state(null);
  let videoElement: HTMLVideoElement | null = $state(null);
  let regions: RegionsPlugin | null = $state(null);
  let wavesurfer = $state<WaveSurfer>();
  let isPlaying = $state(false);
  let editingRegionName = $state("");
  let editingTag: Tag | null = $state(null);
  let tags: Tag[] = $state([]);
  let tagDialog = $state<HTMLDialogElement | null>(null);
  let tagManageMode: "add" | "edit" | null = $state("add");
  let newTagName = $state("");
  let newTagColor = $state("");
  let selectedTag: Tag | null = $state(null);
  let regionIds = $state<string[]>([]);
  let regionManageMode: "add" | "edit" | null = $state("add");
  let selectedRegionId: string | null = $state(null);
  let logs: string[] = $state([]);
  let logPanelBody: HTMLDivElement | null = $state(null);
  let exportDialog: HTMLDialogElement | null = $state(null);
  let exportMode: "index_tag_name" | "tag_name" | "start_tag_name" | "name" = $state("index_tag_name");
  let exportPath: string | null = $state(null);
  let exportProgress = $state(0);
  let exportingDialog: HTMLDialogElement | null = $state(null);
  let shortcutDialog: HTMLDialogElement | null = $state(null);

  const PEAKS_COUNT = 2000;
  let peaksChunkUnlisten: (() => void) | null = null;

  // === Shortcut System ===
  const SHORTCUT_ACTIONS = [
    { id: 'playPause'    as const, label: 'Play / Pause',               defaultKey: ' '          },
    { id: 'stepBack'    as const, label: 'Step Back (-0.5s)',            defaultKey: 'arrowleft'  },
    { id: 'stepForward' as const, label: 'Step Forward (+0.5s)',         defaultKey: 'arrowright' },
    { id: 'jumpToStart' as const, label: 'Jump to Start Marker',        defaultKey: 'w'          },
    { id: 'tempStart'   as const, label: 'Set Temp Start',              defaultKey: 'q'          },
    { id: 'tempEnd'     as const, label: 'Set Temp End',                defaultKey: 'e'          },
    { id: 'playTemp'    as const, label: 'Play Temp Region',            defaultKey: 's'          },
    { id: 'resetTemp'   as const, label: 'Reset Temp Region',           defaultKey: 'r'          },
    { id: 'addRegion'   as const, label: 'Add / Edit Region',           defaultKey: 'f'          },
    { id: 'focusTag'    as const, label: 'Focus Tag Selector',          defaultKey: 'a'          },
    { id: 'focusName'   as const, label: 'Focus Region Name Input',     defaultKey: 'd'          },
  ] as const;

  type ActionId = typeof SHORTCUT_ACTIONS[number]['id'];
  type ShortcutConfig = Record<ActionId, string>;

  const DEFAULT_SHORTCUTS: ShortcutConfig = Object.fromEntries(
    SHORTCUT_ACTIONS.map(a => [a.id, a.defaultKey])
  ) as ShortcutConfig;

  let shortcuts: ShortcutConfig = $state({ ...DEFAULT_SHORTCUTS });
  let rebindingId: ActionId | null = $state(null);
  let tagSelectEl: HTMLSelectElement | null = $state(null);
  let regionNameEl: HTMLInputElement | null = $state(null);

  // 同じキーに複数のアクションが登録されているアクションIDのセット
  let conflictIds = $derived((() => {
    const keyCount = new Map<string, ActionId[]>();
    for (const action of SHORTCUT_ACTIONS) {
      const k = shortcuts[action.id];
      if (!keyCount.has(k)) keyCount.set(k, []);
      keyCount.get(k)!.push(action.id);
    }
    return new Set([...keyCount.values()].filter(ids => ids.length > 1).flat());
  })());

  $effect(() => {
    if (tagManageMode === "edit"){
      if (!selectedTag) return;
      newTagName = selectedTag.name;
      newTagColor = rgbaToHex(selectedTag.color);
    } else {
      newTagName = "";
      newTagColor = randomColor();
      selectedTag = null;
    }
  });

  $effect(() => {
    untrack(() => regionIds.forEach(id => setRegionHidden(id, false)));
    if (selectedRegionId){
      const r = regions?.getRegions().find(r => r.id === selectedRegionId);
      if (r){
        editingRegionName = r.content?.textContent || "";
        editingTag = tags.find(t => t.color === r.color) || null;
        setRegionHidden(r.id, true);
        untrack(() => {
          tempRegionStart(r.start);
          tempRegionEnd(r.end);
        });
        return;
      }
    }
    editingRegionName = "";
    editingTag = null;
    untrack(() => resetTempRegion());
  });

  function tryCloseShortcutDialog() {
    rebindingId = null;
    if (conflictIds.size > 0) {
      const names = [...conflictIds]
        .map(id => SHORTCUT_ACTIONS.find(a => a.id === id)!.label)
        .join('\n  ・ ');
      alert(`キー競合があります。解消してから閉じてください：\n  ・ ${names}`);
      return;
    }
    shortcutDialog?.close();
  }

  function keyLabel(k: string): string {
    const MAP: Record<string, string> = {
      ' ': 'Space', arrowleft: '←', arrowright: '→', arrowup: '↑', arrowdown: '↓',
      escape: 'Esc', enter: 'Enter', backspace: 'BS', delete: 'Del', tab: 'Tab',
    };
    return MAP[k] ?? (k.length === 1 ? k.toUpperCase() : k);
  }

  async function loadShortcuts() {
    try {
      const json = await readTextFile('shortcuts.json', { baseDir: BaseDirectory.AppData });
      const saved: Partial<ShortcutConfig> = JSON.parse(json);
      shortcuts = { ...DEFAULT_SHORTCUTS, ...saved };
      pushLog('Shortcuts loaded.');
    } catch {
      // File not found → use defaults
    }
  }

  async function saveShortcuts() {
    try {
      // ディレクトリが存在しない場合（初回起動時）は先に作成する
      await mkdir('.', { baseDir: BaseDirectory.AppData, recursive: true });
      await writeTextFile('shortcuts.json', JSON.stringify(shortcuts, null, 2), { baseDir: BaseDirectory.AppData });
    } catch (e) {
      console.warn('Failed to save shortcuts:', e);
    }
  }

  function dispatchAction(id: ActionId) {
    switch (id) {
      case 'playPause':    isPlaying ? wavesurfer?.pause() : wavesurfer?.play(); break;
      case 'stepBack':     stepTime(-0.5); break;
      case 'stepForward':  stepTime(0.5); break;
      case 'jumpToStart':  playRegion('start'); break;
      case 'tempStart':    tempRegionStart(); break;
      case 'tempEnd':      tempRegionEnd(); break;
      case 'playTemp':     playRegion('temp'); break;
      case 'resetTemp':    resetTempRegion(); break;
      case 'addRegion':    regionManageMode === 'edit' ? editRegion() : addRegion(); break;
      case 'focusTag':     tagSelectEl?.focus(); break;
      case 'focusName':    regionNameEl?.focus(); regionNameEl?.select(); break;
    }
  }

  function handleKeyDown(ev: KeyboardEvent) {
    if (ev.repeat) return;

    if (rebindingId !== null) {
      ev.preventDefault();
      ev.stopPropagation();
      const k = ev.key.toLowerCase();
      if (k !== 'escape') {
        // Warn if key already bound to another action
        const conflict = SHORTCUT_ACTIONS.find(a => a.id !== rebindingId && shortcuts[a.id] === k);
        if (conflict) pushLog(`[warn] Key "${keyLabel(k)}" already bound to "${conflict.label}". Overriding.`);
        shortcuts = { ...shortcuts, [rebindingId]: k };
        saveShortcuts();
      }
      rebindingId = null;
      return;
    }

    const tag = (ev.target as HTMLElement)?.tagName;
    if (tag === 'INPUT' || tag === 'SELECT' || tag === 'TEXTAREA') {
      if (ev.key === 'Enter' || ev.key === 'Escape') {
        ev.preventDefault();
        (ev.target as HTMLElement).blur();
      }
      return;
    }

    const key = ev.key.toLowerCase();
    for (const [id, bound] of Object.entries(shortcuts)) {
      if (key === bound) {
        ev.preventDefault();
        dispatchAction(id as ActionId);
        return;
      }
    }
  }

  function setRegionHidden(regionId: string, hidden: boolean) {
    const r = regions?.getRegions().find(r => r.id === regionId);
    if (!r || !r.element) return;
    if (hidden) {
      r.element.style.display = "none";
    } else {
      r.element.style.display = "";
    }
  }

  const randomColor = () => {
    const letters = '0123456789ABCDEF';
    let color = '#';
    for (let i = 0; i < 6; i++) {
      color += letters[Math.floor(Math.random() * 16)];
    }
    return color;
  };

  function rgbaToHex(rgba: string): string {
    const m = rgba.match(/rgba?\(\s*([0-9]+)\s*,\s*([0-9]+)\s*,\s*([0-9]+)(?:\s*,\s*([0-9.]+)\s*)?\)/i);
    if (!m) throw new Error("Invalid rgba() format");
    const [_, r, g, b, a] = m;
    const to2 = (n: string) => Number(n).toString(16).padStart(2, "0");
    const hex = `#${to2(r)}${to2(g)}${to2(b)}`;
    return hex;
  }

  function hexToRgba(hex: string, alpha = 0.5): string {
    const normalized = hex.replace(/^#?([0-9a-fA-F]{3})$/, (_, s) =>
      "#" + [...s].map((c) => c + c).join("")
    );
    const m = normalized.match(/^#?([0-9a-fA-F]{6})$/);
    if (!m) throw new Error("Invalid hex color");
    const int = parseInt(m[1], 16);
    const r = (int >> 16) & 0xff;
    const g = (int >> 8) & 0xff;
    const b = int & 0xff;
    return `rgba(${r}, ${g}, ${b}, ${alpha})`;
  }

  function getCssProp(prop: string): string {
    return getComputedStyle(document.documentElement).getPropertyValue(prop).trim();
  }

  function pushLog(msg: string) {
    const t = new Date().toLocaleTimeString();
    logs = [...logs, `${t} ${msg}`];
    // auto-scroll
    setTimeout(() => {
      logPanelBody?.scrollTo({ top: logPanelBody.scrollHeight, behavior: 'smooth' });
    }, 0);
  }

  onMount(() => {
    loadShortcuts();
    window.addEventListener('keydown', handleKeyDown);

    regions = RegionsPlugin.create();
    wavesurfer = WaveSurfer.create({
      container: '#waveform',
      waveColor: '#a0b4ff',
      progressColor: '#6c81f8',
      cursorColor: '#333',
      height: 80,
      normalize: true,
      plugins: [
        regions,
        ZoomPlugin.create({
          maxZoom: 100,
          scale: 0.1,
        }),
        TimelinePlugin.create()
      ],
      media: videoElement || undefined,
      mediaControls: true,
      minPxPerSec: 100,
    });
    wavesurfer.on('play', () =>{
      isPlaying = true;
      if (!regions) return;
      const startMarker = regions.getRegions().find(r => r.id === 'start');
      if (startMarker){
        startMarker.setOptions({start: wavesurfer!.getCurrentTime()});
      }else{
        regions.addRegion({
          id: 'start',
          start: wavesurfer!.getCurrentTime(),
          color: 'rgba(255, 123, 0, 0.3)',
          drag: false,
          resize: false
        });
      }
    });
    wavesurfer.on('pause', () => {
      isPlaying = false;
    });
    regions.on('region-created', (region) => {
      updateRegionIds();
      if (!region.element) return;
      const label = region.element.querySelector('[part="region-content"]') as HTMLElement | null;
      if (!label) return;

      label.style.opacity = '0';
      label.style.transition = 'opacity 0.14s ease';
      label.style.textWrap = 'nowrap';

      region.element.addEventListener('mouseenter', () => {
        label.style.opacity = '1';
      });
      region.element.addEventListener('mouseleave', () => {
        label.style.opacity = '0';
      });
    });
    regions.on('region-removed', () => updateRegionIds());
  });

  function updateRegionIds() {
    if (!regions) return;
    regionIds = regions.getRegions()
      .filter(r => r.id !== 'start' && r.id !== 'temp')
      .sort((a, b) => a.start - b.start)
      .map(r => r.id);
  }

  async function initValues() {
    // 進行中のピーク生成リスナーを解除
    peaksChunkUnlisten?.();
    peaksChunkUnlisten = null;
    fileName = "No file selected.";
    if (videoElement) {
      videoElement.src = "";
    }
    regions?.clearRegions();
    wavesurfer?.empty();
    isPlaying = false;
    editingRegionName = "";
    tags = [];
    editingTag = null;
    regionManageMode = "add";
    selectedRegionId = null;
  }

  async function openFile(){
    initValues();
    const input = await open({
      title: "Select an Video or Audio file",
      filters: [
        { name: "Video/Audio", extensions: ["mp4", "mp3", "wav"] },
      ],
      multiple: false,
    });
    if (!input) {
      fileName = "No file selected.";
      pushLog("File selection cancelled.");
      return;
    }
    inputPath = input as string;
    fileName = inputPath.split(/[\\/]/).pop() || fileName;
    pushLog(`Selected file: ${fileName}`);
    await loadSourceFromPath(inputPath);
  }

  // Tauri v2 のカスタムプロトコル URL はプラットフォームによって異なる
  // Windows (WebView2): http://SCHEME.localhost/path
  // macOS / Linux:      SCHEME://localhost/path
  const isWindows = navigator.userAgent.toLowerCase().includes('windows nt');
  const STREAM_BASE = isWindows
    ? 'http://stream.localhost/video'
    : 'stream://localhost/video';

  // ファイルを切り替えるたびに新しい URL を生成してブラウザキャッシュを無効化する
  let streamUrl = STREAM_BASE;

  async function loadSourceFromPath(path: string) {
    pushLog("Loading media...");
    try {
      await invoke('set_video_path', { path });
      if (!videoElement) throw new Error('Video element not found');

      // ファイルごとに異なる URL を生成してブラウザの Range キャッシュをリセット
      streamUrl = `${STREAM_BASE}?v=${Date.now()}`;
      const allPeaks = new Float32Array(PEAKS_COUNT);
      let wavesurferReady = false;

      // peaks-chunk イベントを先に購読してからピーク生成を開始する
      peaksChunkUnlisten = await listen<{
        peaks: number[];
        offset: number;
        total: number;
        duration: number;
        done: boolean;
      }>('peaks-chunk', ({ payload }) => {
        // チャンクをバッファへ書き込み
        const end = Math.min(payload.offset + payload.peaks.length, allPeaks.length);
        for (let i = payload.offset; i < end; i++) {
          allPeaks[i] = payload.peaks[i - payload.offset];
        }

        if (wavesurferReady) {
          // WaveSurfer の内部 AudioBuffer をインプレース更新 → メディア再読み込みなし
          const w = wavesurfer as any;
          if (w?.decodedData) {
            w.decodedData.getChannelData(0).set(allPeaks);
            w.renderer.render(w.decodedData);
          }
        }
        // wavesurferReady == false の場合はチャンクが allPeaks に蓄積されるのみ。
        // 後の wavesurfer.load() 呼び出し時にまとめて反映される。

        if (payload.done) {
          peaksChunkUnlisten?.();
          peaksChunkUnlisten = null;
          if (wavesurferReady) pushLog('Waveform complete.');
        }
      });

      // バックグラウンドでピーク生成開始 (await しない)
      invoke('generate_peaks', { path, peaksCount: PEAKS_COUNT })
        .catch(e => pushLog(`Waveform error: ${e}`));

      // 動画ソースをセットしてメタデータが届くのを待つ
      videoElement.src = streamUrl;
      const duration = await new Promise<number>(resolve => {
        if (videoElement && isFinite(videoElement.duration) && videoElement.duration > 0) {
          resolve(videoElement.duration);
        } else {
          videoElement!.addEventListener('loadedmetadata', () => {
            resolve(videoElement!.duration);
          }, { once: true });
        }
      });

      // WaveSurfer を初期化 (蓄積済みピーク付き)
      await wavesurfer?.load(streamUrl, [allPeaks], duration);
      wavesurferReady = true;
      pushLog(`Media loaded (${(duration / 60).toFixed(1)} min). Generating waveform...`);
    } catch (error) {
      console.error("Error loading media:", error);
      pushLog(`Error loading media: ${error}`);
    }
  }

  function stepTime(step: number) {
    if (!wavesurfer) return;
    const current = wavesurfer.getCurrentTime();
    let newTime = Math.max(Math.min(current + step, wavesurfer.getDuration()), 0);
    wavesurfer.seekTo(newTime / wavesurfer.getDuration());
  }

  function tempRegionStart(time?: number) {
    if (!wavesurfer || !regions) return;
    let tempRegion = regions.getRegions().find(r => r.id === 'temp');
    const start = time || wavesurfer.getCurrentTime();
    if (!tempRegion) {
      tempRegion = regions.addRegion({
        id: 'temp',
        start,
        end: start + 1,
        color: 'rgba(0, 123, 255, 0.3)',
        drag: false,
        resize: true
      });
      pushLog(`Temp region started at ${start.toFixed(2)}s`);
    } else {
      tempRegion.setOptions({ start });
      if (tempRegion.end <= start) {
        tempRegion.setOptions({ end: start + 1 });
      }
      pushLog(`Temp region updated start to ${start.toFixed(2)}s`);
    }
  }

  function tempRegionEnd(time?: number) {
    if (!wavesurfer || !regions) return;
    const tempRegion = regions.getRegions().find(r => r.id === 'temp');
    if (!tempRegion) return;
    const end = time || wavesurfer.getCurrentTime();
    if (end > tempRegion.start) {
      tempRegion.setOptions({ end });
      pushLog(`Temp region end set to ${end.toFixed(2)}s`);
    }
  }

  function addRegion() {
    if (!wavesurfer || !regions || editingRegionName == "") return false;
    const tempRegion = regions.getRegions().find(r => r.id === 'temp');
    if (!tempRegion) return false;
    const color = editingTag ? editingTag.color : getCssProp('--default-region-color');
    regions.addRegion({
      start: tempRegion.start,
      end: tempRegion.end,
      color: color,
      drag: false,
      resize: false,
      content: editingRegionName,
    });
    pushLog(`Added region "${editingRegionName}" (${tempRegion.start.toFixed(2)}s - ${tempRegion.end?.toFixed(2)}s)`);
    resetTempRegion();
    return true;
  }

  function editRegion() {
    if (!wavesurfer || !regions || editingRegionName == "") return;
    const r = regions.getRegions().find(r => r.id === selectedRegionId);
    if (!r || !addRegion()) return;
    r.remove();
  }

  function playRegion(id: string) {
    if (!wavesurfer || !regions) return;
    const region = regions.getRegions().find(r => r.id === id);
    if (!region) return;
    if (region.end - region.start <= 0.1) {
      wavesurfer.play(region.start);
    }else{
      wavesurfer.play(region.start, region.end);
    }
  }

  function removeRegion(id: string) {
    if (!regions) return;
    const r = regions.getRegions().find(r => r.id === id);
    if (r) {
      r.remove();
      pushLog(`Removed region "${r.content?.textContent ?? "Unnamed"}" (${r.start.toFixed(2)}s - ${r.end?.toFixed(2)}s)`);
      if (selectedRegionId === id) {
        selectedRegionId = null;
      }
    }
  }

  function resetTempRegion() {
    if (!regions) return;
    const tempRegion = regions.getRegions().find(r => r.id === 'temp');
    if (tempRegion) {
      tempRegion.remove();
      pushLog('Temp region reset');
    }
    editingRegionName = "";
    selectedRegionId = null;
  }

  function manageTag(){
    const newRgba = hexToRgba(newTagColor);
    if (tagManageMode === "add") {
      if (newTagName.trim() === "") return;
      if (tags.some(tag => tag.name === newTagName)) {
        alert("Tag name already exists.");
        return;
      }
      if (tags.some(tag => tag.color === newRgba)) {
        alert("Tag color already exists.");
        return;
      }
      tags = [...tags, { name: newTagName, color: newRgba }];
      pushLog(`Tag added: ${newTagName}`);
      newTagName = "";
      newTagColor = randomColor();
    } else if (tagManageMode === "edit" && selectedTag) {
      if (newTagName.trim() === "") return;
      if (tags.some(tag => tag.name === newTagName && tag !== selectedTag)) {
        alert("Tag name already exists.");
        return;
      }
      if (tags.some(tag => tag.color === newRgba && tag !== selectedTag)) {
        alert("Tag color already exists.");
        return;
      }
      const newTag: Tag = { name: newTagName, color: newRgba };
      tags = tags.map(tag => tag === selectedTag ? newTag : tag);
      replaceTag(selectedTag, newTag);
      pushLog(`Tag edited: ${selectedTag.name} -> ${newTagName}`);
      selectedTag = null;
      newTagName = "";
      newTagColor = randomColor();
    }
  }

  function replaceTag(oldTag: Tag, newTag: Tag | null) {
    if (!regions) return;
    const allRegions = regions.getRegions();
    allRegions.forEach(region => {
      if (region.color === oldTag.color) {
        region.setOptions({ color: newTag ? newTag.color : getCssProp('--default-region-color') });
      }
    });
    if (editingTag === oldTag) {
      editingTag = newTag;
    }
    pushLog(`Replaced tag colors: ${oldTag.name} -> ${newTag?.name ?? 'default'}`);
  }

  async function selectExportPath() {
    const path = await open({
      title: "Export directory",
      directory: true,
      multiple: false
    });
    if (path) {
      pushLog(`Selected export path: ${path}`);
      exportPath = path as string;
    } else {
      pushLog("Export path selection cancelled.");
    }
  }

  async function exportRegions() {
    if (!regions || regionIds.length === 0) {
      alert("No regions to export.");
      return;
    }
    if (!inputPath) {
      alert("No input file selected.");
      return;
    }
    if (!exportPath) {
      alert("Please select an export path.");
      return;
    }
    const nameCount = new Map<string, number>();
    const exportData = regionIds.map((id, index) => {
      const r = regions?.getRegions().find(r => r.id === id);
      if (!r) throw new Error("Region not found: " + id);
      const tag = tags.find(t => t.color === r.color);
      const tagName = tag ? tag.name : "";
      const name = r.content?.textContent ?? "";
      let fileName = "";
      switch (exportMode) {
        case "index_tag_name":
          fileName = `${index + 1}_${tagName}_${name}`;
          break;
        case "tag_name":
          fileName = `${tagName}_${name}`;
          break;
        case "start_tag_name":
          fileName = `${r.start.toFixed(2)}_${tagName}_${name}`;
          break;
        case "name":
          fileName = name;
          break;
      }
      const baseName = fileName;
      const count = (nameCount.get(baseName) ?? 0) + 1;
      nameCount.set(baseName, count);
      if (count > 1) {
        fileName = `${baseName}_${count}`;
      }
      return {
        name: fileName,
        start: r.start,
        end: r.end
      };
    });
    let unlistenProgress: (() => void) | null = null;
    let unlistenLog: (() => void) | null = null;
    try {
      unlistenProgress = await listen<number>("export-progress", (event) => {
        exportProgress = event.payload;
      });
      unlistenLog = await listen<string>("export-log", (event) => {
        pushLog(event.payload);
      });
      exportingDialog?.showModal();
      await invoke('export_regions', { inputPath, outDir: exportPath, regions: exportData });
      pushLog(`Exported ${exportData.length} regions to ${exportPath}`);
    } catch (error) {
      console.error("Error exporting regions:", error);
      pushLog(`Error exporting regions: ${error}`);
    } finally {
      unlistenProgress?.();
      unlistenLog?.();
      exportingDialog?.close();
      exportDialog?.close();
    }
  }
</script>

<main class="container">
  <div class="left">
    <div class="controls">
      <button onclick={openFile} title="import file"><FileDown size="16"/></button>
      <span class="file-name">{fileName}</span>
    </div>
    <video bind:this={videoElement} class="preview"></video>
    <div id="waveform" class="waveform" role="button"></div>
    <div class="controls">
      <button onclick={() => isPlaying ? wavesurfer?.pause() : wavesurfer?.play()} title="play/pause">
        {#if isPlaying}
          <Pause size="16"/>
        {:else}
          <Play size="16"/>
        {/if}
      </button>
      <button onclick={() => stepTime(-0.5)} title="Move -0.5s"><StepBack size="16"/></button>
      <button onclick={() => stepTime(0.5)} title="Move +0.5s"><StepForward size="16"/></button>
      <button onclick={() => playRegion("start")} title="Jump to start marker"><SkipBack size="16"/></button>
    </div>
    <div class="controls">
      <button onclick={() => tempRegionStart()} title="Set temp start"><ArrowLeftToLine size="16"/></button>
      <button onclick={() => tempRegionEnd()} title="Set temp end"><ArrowRightToLine size="16"/></button>
      <button onclick={() => playRegion("temp")} title="Play temp region"><Disc size="16"/></button>
      <button onclick={resetTempRegion} title="Reset temp region"><Trash2 size="16"/></button>
      <div class="tag-controls">
        <div class="box" style="background-color:{editingTag ? editingTag.color : undefined};"></div>
        <select bind:value={editingTag} bind:this={tagSelectEl}>
          <option value={null}>None</option>
          {#each tags as tag}
            <option value={tag} style="background-color: {tag.color};">{tag.name}</option>
          {/each}
        </select>
        <button onclick={() => {
          if (tagDialog) {
            tagDialog.showModal();
            newTagColor = randomColor();
            tagManageMode = "add";
          }
        }}><Tag size="16"/></button>
      </div>
    </div>
    <input style="width: calc(100% - 16px);" type="text" title="Region name" placeholder="Region Name" bind:value={editingRegionName} bind:this={regionNameEl}/>
    {#if regionManageMode === "edit"}
      <button onclick={editRegion} style="width: calc(100% - 8px)" title="Apply changes" disabled={!selectedRegionId || editingRegionName == ""}>
        EditRegion
      </button>
    {:else}
      <button onclick={addRegion} style="width: calc(100% - 8px)" title="Add region" disabled={editingRegionName == ""}>
        AddRegion
      </button>
    {/if}
    <div class="controls">
      <button onclick={() => shortcutDialog?.showModal()} title="Keyboard Shortcuts"><Keyboard size="16"/></button>
    </div>
  </div>
  <div class="right">
    <div class=regions>
      <div class=controls>
        <select bind:value={regionManageMode} onchange={() => selectedRegionId = null}>
          <option value="add">Add Region</option>
          <option value="edit">Edit Region</option>
        </select>
        <button onclick={() =>{
          if (exportDialog) {
            exportDialog.showModal();
            exportProgress = 0;
          }
        }}>
          Export All
        </button>
      </div>
      <div class="list">
        {#each regionIds as id}
          {@const r = regions?.getRegions().find(r => r.id === id)}
          <button class="panel" data-select={selectedRegionId==id} onclick={() =>{
            if (regionManageMode == "edit") {
              selectedRegionId = id;
              if (r) {
                editingRegionName = r.content?.textContent || "";
                editingTag = tags.find(t => t.color === r.color) || null;
              }
            }
          }}>
            <Play size="16" onclick={() => r && playRegion(r.id)}/>
            <div class="box" style="background-color:{r?.color ?? undefined}"></div>
            <span>{r?.content?.textContent ?? "Unnamed"}</span>
            <span>{r?.start.toFixed(2) + "~" + r?.end.toFixed(2)}</span>
            <Trash2 size="16" onclick={() => {r && removeRegion(r.id)}}/>
          </button>
        {/each}
      </div>
    </div>
    <div class="log">
      <div class="header">Logs</div>
      <div class="body" bind:this={logPanelBody}>
        {#each logs as l, i (i)}
          <div class="line">{l}</div>
        {/each}
      </div>
    </div>
  </div>
</main>
<dialog bind:this={tagDialog}>
  <div class="tag-dialog">
    <select bind:value={tagManageMode}>
      <option value="add">Add Tag</option>
      <option value="edit">Edit Tag</option>
    </select>
    <div class="controls">
      <input type="text" bind:value={newTagName}/>
      <input type="color" bind:value={newTagColor}/>
      <button onclick={manageTag} disabled={tagManageMode=="edit" && selectedTag==null}>{tagManageMode}</button>
    </div>
    <div class="list">
      {#each tags as tag}
        <button class="panel" data-select={selectedTag==tag} onclick={() =>{
          if (tagManageMode == "edit") {
            selectedTag = tag;
          }
        }}>
          <div class="box" style="background-color:{tag.color}"></div>
          <span>{tag.name}</span>
        </button>
      {/each}
    </div>
    <button onclick={() => tagDialog?.close()}>Apply</button>
  </div>
</dialog>

<dialog bind:this={exportDialog}>
  <div class="export-dialog">
    <div class="controls">
      <button onclick={selectExportPath}><Folder size="16"/></button>
      <span class="file-name">{exportPath ?? "chose export path"}</span>
    </div>
    <div class=controls>
      ExportName: 
      <select bind:value={exportMode}>
        <option value="index_tag_name">index_tag_name</option>
        <option value="tag_name">tag_name</option>
        <option value="start_tag_name">start_tag_name</option>
        <option value="name">name</option>
      </select>
    </div>
    <div class=controls>
      <button onclick={() => exportDialog?.close()}>Close</button>
      <button onclick={exportRegions} style="margin-left: auto;" disabled={!exportPath || regionIds.length == 0}>Export</button>
    </div>
  </div>
</dialog>

<dialog bind:this={exportingDialog}>
  <div class="exporting-dialog">
    <span>Exporting... Please wait.</span>
    <progress value={exportProgress} max="100" style="width: 100%"></progress>
  </div>
</dialog>

<dialog bind:this={shortcutDialog} oncancel={(e) => { e.preventDefault(); tryCloseShortcutDialog(); }}>
  <div class="shortcut-dialog">
    <h3>Keyboard Shortcuts</h3>
    <div class="shortcut-list">
      {#each SHORTCUT_ACTIONS as action}
        <div class="shortcut-row" data-conflict={conflictIds.has(action.id)}>
          <span class="shortcut-label">{action.label}</span>
          <button
            class="shortcut-key"
            data-rebinding={rebindingId === action.id}
            onclick={() => rebindingId = action.id}
          >{rebindingId === action.id ? 'Press key...' : keyLabel(shortcuts[action.id])}</button>
          <button
            class="shortcut-reset"
            title="Reset to default"
            disabled={shortcuts[action.id] === action.defaultKey}
            onclick={() => { shortcuts = { ...shortcuts, [action.id]: action.defaultKey }; saveShortcuts(); }}
          >↩</button>
        </div>
      {/each}
    </div>
    <div class="controls" style="margin-top: 8px;">
      <button onclick={() => { shortcuts = { ...DEFAULT_SHORTCUTS }; saveShortcuts(); }}>Reset All</button>
      <button onclick={tryCloseShortcutDialog} style="margin-left: auto;" disabled={conflictIds.size > 0} title={conflictIds.size > 0 ? 'キー競合を解消してください' : ''}>Close</button>
    </div>
  </div>
</dialog>

<style>
:root {
  --default-bg: #f6f6f6;
  --selected-bg: #a0b4ff;
  --default-region-color: rgba(184, 184, 184, 0.5);
}

.container {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  height: calc(100vh - 16px);
}

.controls { 
  display:flex; 
  gap:8px; 
  align-items:left; 
}

.left {
  border-right: 1px solid #0f0f0f;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.file-name {
  font-size: 14px;
  color: #333;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.preview {
  height: 160px;
  width: fit-content;
  background: #1b1b1b;
  border: 1px solid #0f0f0f;
  border-radius: 6px;
  object-fit: contain;
}

.waveform { 
  border:1px solid #1f1f1f; 
  border-radius:6px; 
  width: calc(100% - 8px); 
  height: fit-content; 
  overflow-x: auto; 
  min-width: 0; 
  min-height: 80px;
}

.box {
  border: 1px solid #1d1d1d;
  aspect-ratio: 1 / 1;
}

.tag-controls {
  display: flex;
  flex-direction: row;
  gap: 0px;
  & .box {
    background-color: var(--default-region-color);
  }
  & select {
    border: 1px solid #1d1d1d;
    border-right: none;
    border-left: none;
    padding: 4px;
    max-width: 160px;
  }
}

.right {
  display: grid;
  padding-left: 8px;
  grid-template-rows: minmax(0, 3fr) minmax(0, 1fr);
  height: 100%;
  min-height: 0;
}

.list {
  display: flex;
  flex-direction: column;
  gap: 4px;
  overflow-y: auto;
  & .panel {
    border: 1px solid #1d1d1d;
    border-radius: 4px;
    padding: 4px;
    text-align: left;
    display:flex; 
    align-items:center; 
    gap:4px; 
    background-color: var(--default-bg);
    &[data-select="true"] {
      background-color: var(--selected-bg);
    }
    & .box {
      height: 16px;
    }
  }
}

.regions {
  display: flex;
  flex-direction: column;
  & .controls {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 0;
    & button {
      margin-left: auto;
    }
  }
  & .list .panel{
      display: grid;
      grid-template-columns: auto auto 1fr auto auto;
  }
}

.log {
    background: #0f1724;
    color: #e6eef8;
    border: 1px solid #2b3a4a;
    font-family: monospace;
    font-size: 12px;
    display: flex;
    flex-direction: column;
  & .header { 
    padding: 6px 8px; 
    background: #0b1220; 
    border-bottom: 1px solid #1f2a38; 
    font-weight: 600; 
  }
  & .body {
    padding: 8px; 
    max-height: 100%;
    overflow-y: auto;
  }
  & .line { 
    margin-bottom: 4px; 
    white-space: pre-wrap; 
    color: #cfe7ff; 
  }
}
.tag-dialog {
  display: flex;
  flex-direction: column;
  gap: 8px;
  & .controls {
    gap: 4px;
  }
  & .list {
    max-height: 200px;
    width: 250px;
  }
}
.export-dialog {
  width : 300px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.exporting-dialog {
  display: flex;
  flex-direction: column;
  gap: 8px;
  width: 250px;
}
.shortcut-dialog {
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-width: 360px;
  & h3 { margin: 0 0 4px 0; }
  & .shortcut-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  & .shortcut-row {
    display: grid;
    grid-template-columns: 1fr auto auto;
    align-items: center;
    gap: 8px;
    padding: 2px 4px;
    border-radius: 4px;
    &[data-conflict="true"] {
      background: rgba(220, 38, 38, 0.12);
      outline: 1px solid rgba(220, 38, 38, 0.5);
      & .shortcut-label { color: #c00; font-weight: 600; }
      & .shortcut-key { border-color: #c00; }
    }
  }
  & .shortcut-key {
    min-width: 96px;
    font-family: monospace;
    text-align: center;
    &[data-rebinding="true"] {
      background: #ffe0b2;
      border-color: #ff9800;
      animation: blink 0.8s step-start infinite;
    }
  }
  & .shortcut-reset {
    padding: 2px 6px;
  }
}
@keyframes blink {
  50% { opacity: 0.4; }
}
</style>
