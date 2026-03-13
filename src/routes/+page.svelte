<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from '@tauri-apps/plugin-dialog';
  import { readFile } from '@tauri-apps/plugin-fs';
  import WaveSurfer from 'wavesurfer.js';
  import RegionsPlugin from 'wavesurfer.js/dist/plugins/regions.esm.js';
  import ZoomPlugin from 'wavesurfer.js/dist/plugins/zoom.esm.js';
  import TimelinePlugin from 'wavesurfer.js/dist/plugins/timeline.esm.js'
  import { onMount } from "svelte";
  import {
    FileDown, Play, Pause, StepBack, StepForward, SkipBack,
    ArrowLeftToLine, ArrowRightToLine, Disc, Trash2, Tag
  } from "@lucide/svelte";

  type Tag = {
    name: string;
    color: string;
  };

  let fileName = $state("No file selected");
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

  $effect(() => {
    if (tagManageMode === "edit" && selectedTag) {
      newTagName = selectedTag.name;
      newTagColor = rgbaToHex(selectedTag.color);
    } else {
      newTagName = "";
      newTagColor = randomColor();
      selectedTag = null;
    }
  });

  $effect(() => {
    if (regionManageMode === "edit" && selectedRegionId && regions) {
      const r = regions.getRegions().find(r => r.id === selectedRegionId);
      if (r) {
        editingRegionName = r.content?.textContent || "";
        editingTag = tags.find(t => t.color === r.color) || null;
      }
    } else {
      editingRegionName = "";
      editingTag = null;
      selectedRegionId = null;
    }
  });

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
    regions.on('region-created', () => updateRegionIds());
    regions.on('region-updated', () => updateRegionIds());
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
    fileName = "No file selected.";
    if (videoElement) {
      videoElement.src = "";
    }
    wavesurfer?.empty();
    isPlaying = false;
    editingRegionName = "";
    tags = [];
  }

  async function openFile(){
    initValues();
    const inputPath = await open({
      title: "Select an Video or Audio file",
      filters: [
        { name: "Video/Audio", extensions: ["mp4", "mp3", "wav"] },
      ],
      multiple: false,
    });
    if (!inputPath) {
      fileName = "No file selected.";
      pushLog("File selection cancelled.");
      return;
    }
    fileName = inputPath.split(/[\\/]/).pop() || fileName;
    pushLog(`Selected file: ${fileName}`);
    await loadSourceFromPath(inputPath);
  }

  async function loadSourceFromPath(path: string) {
    pushLog("Loading media...");
    try {
      const bin = await readFile(path as string);
      let arrayBuffer: ArrayBuffer;
      if (bin === null) {
        throw new Error('File data is null');
      }
      if (bin instanceof Uint8Array) {
        arrayBuffer = bin.buffer.slice(bin.byteOffset, bin.byteOffset + bin.byteLength);
      } else if (Array.isArray(bin)) {
        arrayBuffer = new Uint8Array(bin as number[]).buffer;
      } else if ((bin as any) instanceof ArrayBuffer) {
        arrayBuffer = bin;
      } else {
        throw new Error('Unsupported file data type');
      }
      const ext = path.split('.').pop()?.toLowerCase() || '';
      const isVid = ['mp4'].includes(ext);
      const blob = new Blob([arrayBuffer], { type: isVid ? 'video/*' : 'audio/*' });
      const url = URL.createObjectURL(blob);
      if (videoElement) {
        videoElement.src = url;
        videoElement.load();
      }else {
        throw new Error('Video element not found');
      }
      wavesurfer?.load(url);
      pushLog("Media loaded.");
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

  function tempRegionStart(){
    if (!wavesurfer || !regions) return;
    let tempRegion = regions.getRegions().find(r => r.id === 'temp');
    const start = wavesurfer.getCurrentTime();
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

  function tempRegionEnd(){
    if (!wavesurfer || !regions) return;
    const tempRegion = regions.getRegions().find(r => r.id === 'temp');
    if (!tempRegion) return;
    const end = wavesurfer.getCurrentTime();
    if (end > tempRegion.start) {
      tempRegion.setOptions({ end });
      pushLog(`Temp region end set to ${end.toFixed(2)}s`);
    }
  }

  function addRegion() {
    if (!wavesurfer || !regions || editingRegionName == "") return;
    const tempRegion = regions.getRegions().find(r => r.id === 'temp');
    if (!tempRegion) return;
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
  }

  function playRegion(id: string) {
    if (!wavesurfer || !regions) return;
    const region = regions.getRegions().find(r => r.id === id);
    if (!region) return;
    pushLog(`Playing region ${id} (${region.start.toFixed(2)}s - ${region.end?.toFixed(2) ?? 'end'}s)`);
    if (!region.end) {
      wavesurfer.play(region.start);
    }else{
      wavesurfer.play(region.start, region.end);
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
      <button title="Jump to start marker"><SkipBack size="16"/></button>
    </div>
    <div class="controls">
      <button onclick={tempRegionStart} title="Set temp start"><ArrowLeftToLine size="16"/></button>
      <button onclick={tempRegionEnd} title="Set temp end"><ArrowRightToLine size="16"/></button>
      <button onclick={() => playRegion("temp")} title="Play temp region"><Disc size="16"/></button>
      <button onclick={resetTempRegion} title="Reset temp region"><Trash2 size="16"/></button>
      <div class="tag-controls">
        <div class="box" style="background-color:{editingTag ? editingTag.color : undefined};"></div>
        <select bind:value={editingTag}>
          <option value={null}>None</option>
          {#each tags as tag}
            <option value={tag}>{tag.name}</option>
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
    <input style="width: calc(100% - 16px);" type="text" title="Region name" placeholder="Region Name" bind:value={editingRegionName}/>
    <button onclick={addRegion} style="width: calc(100% - 8px)" title="Add region">
      AddRegion
    </button>
  </div>
  <div class="right">
    <div class=regions>
      <div class=controls>
        <select bind:value={regionManageMode}>
          <option value="add">Add Region</option>
          <option value="edit">Edit Region</option>
        </select>
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
            <Play size="16"/>
            <div class="box" style="background-color:{r?.color ?? undefined}"></div>
            <span>{r?.content?.textContent ?? "Unnamed"}</span>
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
<dialog id="tag-dialog" bind:this={tagDialog}>
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
    padding: 4px 0;
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
</style>
