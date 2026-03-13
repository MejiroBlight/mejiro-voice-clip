<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from '@tauri-apps/plugin-dialog';
  import { readFile } from '@tauri-apps/plugin-fs';
  import WaveSurfer from 'wavesurfer.js';
  import RegionsPlugin, { type RegionParams } from 'wavesurfer.js/dist/plugins/regions.esm.js';
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
  let regions: RegionsPlugin | null;
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
  let extractMsg = $state("");

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

  const selectedColor = "#a0b4ff";
  const nonSelectedColor = "#f6f6f6";
  const defaultRegionColor = "#b8b8b8";

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
    wavesurfer.on('play', () => isPlaying = true);
    wavesurfer.on('pause', () => isPlaying = false);
  });

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
      return;
    }
    fileName = inputPath.split(/[\\/]/).pop() || fileName;
    await loadSourceFromPath(inputPath);
  }

  async function loadSourceFromPath(path: string) {
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
    } catch (error) {
      console.error("Error loading media:", error);
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
    } else {
      tempRegion.setOptions({ start });
      if (tempRegion.end <= start) {
        tempRegion.setOptions({ end: start + 1 });
      }
    }
  }

  function tempRegionEnd(){
    if (!wavesurfer || !regions) return;
    const tempRegion = regions.getRegions().find(r => r.id === 'temp');
    if (!tempRegion) return;
    const end = wavesurfer.getCurrentTime();
    if (end > tempRegion.start) {
      tempRegion.setOptions({ end });
    }
  }

  function addRegion() {
    if (!wavesurfer || !regions || editingRegionName == "") return;
    const tempRegion = regions.getRegions().find(r => r.id === 'temp');
    if (!tempRegion) return;
    const color = editingTag ? editingTag.color : defaultRegionColor;
    regions.addRegion({
      start: tempRegion.start,
      end: tempRegion.end,
      color: color,
      drag: false,
      resize: false,
      content: editingRegionName,
    });
  }

  function handleDialogClick(event: MouseEvent) {
    if (event.target === tagDialog) {
      tagDialog?.close();
    }
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
    }
  }

  function replaceTag(oldTag: Tag, newTag: Tag | null) {

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
      <button title="Play temp region"><Disc size="16"/></button>
      <button title="Reset temp region"><Trash2 size="16"/></button>
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

  </div>
</main>
<dialog id="tag-dialog" bind:this={tagDialog} onclick={handleDialogClick}>
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
  flex-direction: column;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  height: calc(100vh - 16px);
}

.controls { display:flex; gap:8px; align-items:left; }

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

.tag-dialog {
  display: flex;
  flex-direction: column;
  gap: 8px;
  & .controls {
    display: flex;
    flex-direction: row;
    gap: 4px;
  }
  & .list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 200px;
    width: 250px;
    overflow-y: auto;
  }
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
</style>
