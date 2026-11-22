# 04 - Local AI for Cutweave (research, 2026-11-22)

Legend: [V] = verified this session on the model card / repo page. [U] = unverified, from prior knowledge (cutoff mid-2026); re-check before shipping. Sizes are approximate; fp16/int8 ONNX sizes are estimates unless marked [V].

## 0. Executive summary

- Primary runtime: **ort (ONNX Runtime) 2.0.0-rc.x [V rc.15 seen on README; ORT 1.31 badge]** with per-platform execution providers (EPs). Dual MIT/Apache-2.0 [V].
- Exceptions: **whisper.cpp / whisper-rs** (ASR fallback, ggml Vulkan/Metal/CPU, small, one binary), **llama.cpp (llama-cpp-2 crate or sidecar server)** for LLM tasks (GGUF, Vulkan/Metal/CUDA), **sherpa-onnx** (Apache-2.0 [V]; has Rust API [V]) as a convenience bundle for VAD, diarization, TTS, streaming ASR, speech enhancement - it itself runs on ONNX Runtime, so it shares the ORT binary story. candle/burn: not primary (see A).
- Biggest license traps: RMBG-2.0 (CC BY-NC [V]), F5-TTS weights (CC BY-NC [V]), RobustVideoMatting code (GPL-3 [V]), Seed-VC code (GPL-3 [V]), Ultralytics YOLO (AGPL [U]), NLLB (CC BY-NC [U]), Parakeet-v3 / Canary (CC BY 4.0 [V] - OK with attribution, Canary-1B-v1 was CC-BY-NC [U]), aubio (GPL [U]), pyannote pipelines (MIT but gated, and 3.1 depends on gated segmentation-3.0 [V]), SenseVoice (custom "model-license" [V] - read it), XTTS (CPML non-commercial [U]), SDXL (CreativeML OpenRAIL++ [U]), FLUX.1-dev (non-commercial [U]), insightface/SCRFD pretrained models (non-commercial [U]), Llama / Gemma (custom terms, not OSI [U]).

## Part A - Runtimes

| Runtime | Role | Status / notes | License | Verdict |
|---|---|---|---|---|
| ort (pykeio) | Primary | 2.0.0-rc.15 on README [V]; rc status for >1 year, widely used (Bevy, Tauri apps). EPs: CUDA, TensorRT, DirectML, CoreML, ROCm/MIGraphX, OpenVINO, WebGPU, XNNPACK, CPU [EP list per docs, U on exact current set]. Load-dynamic feature lets you ship onnxruntime.dll/.so/.dylib yourself. | MIT/Apache-2.0 [V] | Primary |
| ONNX Runtime binaries | Shipping | CPU-only ORT dll ~10-15 MB; CUDA EP package adds ~300-800 MB incl. cuDNN/cuBLAS (do NOT bundle; download on demand) [U]; DirectML ~ +20 MB and ships with Windows 11 recent builds, needs DirectML.dll [U]; CoreML is built into the macOS dylib; OpenVINO EP ~ +50-100 MB [U]; ROCm EP is Linux-only, MIGraphX Linux; no AMD on Windows via ROCm -> use DirectML (or Vulkan via ggml) [U]. WebGPU EP is new/experimental on desktop [U]. | MIT | Ship CPU+DirectML (Win), CoreML (mac), CPU (Linux) in installer; download "GPU packs" (CUDA/TensorRT/OpenVINO/ROCm) on demand into app data dir and load dynamically. |
| whisper.cpp / whisper-rs | ASR exception | ggml backends: CPU, CUDA, Metal, Vulkan, OpenVINO, CoreML encoder [U]. Vulkan gives AMD/Intel GPU coverage that ORT lacks outside DirectML. | MIT [U] | Use as ASR fallback/ default for non-NVIDIA Linux |
| llama.cpp (llama-cpp-2 crate or llama-server sidecar) | LLM exception | GGUF, Q4_K_M quantization, Vulkan/Metal/CUDA/CPU. ONNX LLM paths (ort-genai) are less mature. | MIT [U] | Use for all LLM tasks (translation, highlights). Prefer sidecar process: isolates crashes/VRAM. |
| sherpa-onnx | Bundle | Apache-2.0 [V]; ASR streaming/non-streaming, diarization, TTS, VAD, speech enhancement [V]; Rust API + rust-api-examples [V]; Win/macOS/Linux x64/arm64 [V]. | Apache-2.0 [V] | Use for VAD, diarization, Kokoro/Piper TTS, Parakeet/Moonshine/SenseVoice ASR if you want one lib. Caveat: bundles its own ORT version - avoid two different ORT dlls in-process (symbol/version clashes); either use sherpa only in a sidecar process or build against shared ORT [U]. |
| candle (HF) | Not primary | Pure Rust, Metal/CUDA, good for Whisper/LLM demos; no DirectML/OpenVINO/ROCm; op coverage per-model hand-ported [U]. | Apache/MIT [U] | Skip except maybe CPU-only tiny models |
| burn | Not primary | Pure Rust, wgpu backend (Vulkan/Metal/DX12) is attractive for single-binary; ONNX import (burn-import) covers limited ops; perf behind ORT for conv-heavy video nets [U]. | Apache/MIT [U] | Watch; revisit 2027 |

**Recommendation:** ort as the single runtime for all vision/audio nets (one model format, ONNX; one EP-selection policy), plus llama.cpp sidecar for LLMs and whisper.cpp as ASR alternative. EP policy at start-up: probe hardware -> order EPs [TensorRT(optional, long engine build; cache) > CUDA > DirectML > CoreML > OpenVINO > CPU]; always keep CPU fallback; benchmark once per model/device and cache choice. Note: DirectML is in maintenance mode (Microsoft steering to WinML/ORT-WinML) [U] - monitor; plan Windows ML EP path.

### Model download / management
- Models are NOT in the installer. Registry JSON (shipped + updatable) listing: id, license SPDX, source URL, sha256, size, ONNX opset, required EP, min VRAM.
- Download from Hugging Face `resolve/<pinned-commit-sha>/file` (pin commit, not `main`) over HTTPS with resume (Range), verify SHA-256 (HF exposes LFS sha256 in file metadata [U]), store content-addressed in `%APPDATA%/Cutweave/models/<id>/<sha>`. Mirror our own converted ONNX files on a Cutweave HF org / GitHub Releases for reproducibility, since many ONNX conversions are third-party.
- Avoid gated repos (pyannote) in default flow; re-host only if license allows redistribution (MIT does, but gating terms are HF-side - mirror the ONNX exports from sherpa-onnx releases instead).
- UI shows license + size before download; license column in registry enforces "redistributable" tag; CI check rejects NC/unknown licenses in default set.
- Total default footprint target: < 1.5 GB for "core" pack (ASR turbo int8 ~0.8 GB, VAD, denoise, segmentation, detector), optional packs on demand.

### Scope update: cloud-first, local as optional "local provider" plugin + offline fallback

Priority for local depth (cheap, high-frequency, privacy/latency-sensitive): (1) ASR with word timestamps, (2) person segmentation / background removal, (3) denoise / voice isolation, (4) scene detection, (5) beat detection, (6) face/object tracking for auto-reframe. Everything else (generative image/video, TTS, voice conversion, translation, LLM highlights) is "brief": one pick each, cloud by default.

Brief picks for the de-prioritized features (licenses per tables below): TTS = Kokoro-82M (Apache-2.0 [V]); voice conversion = plugin only, Chatterbox VC (MIT [V]); translation = Opus-MT/Bergamot per pair, Qwen3-4B (Apache-2.0 [V]) for quality; generative image/video = cloud only, optional local plugin (FLUX.1-schnell / Wan, Apache-2.0 [U]); highlights = cloud LLM, local Qwen3-4B fallback.

Depth notes for the priority six (local recommendation, rationale):
- ASR: ship whisper.cpp (ggml q5/q8, Vulkan/Metal/CUDA/CPU) with large-v3-turbo as the GPU tier and small/base as CPU tier, always behind Silero VAD to cut hallucination and cost; word timestamps from DTW cross-attention (whisper.cpp) or native TDT timestamps from Parakeet-v3 (CC BY 4.0 [V], needs attribution). Output schema must match the cloud STT (words[{text,start,end,conf}], language, speaker optional) so captions are provider-swappable. Run diarization (sherpa-onnx) as a separate step and merge by overlap.
- Segmentation: MODNet/PP-Matting per frame + temporal smoothing (EMA on alpha, optical-flow-free); SAM 2.1-tiny/EdgeTAM for click-to-track. Avoid RMBG-2.0 [V NC] and RVM (GPL-3 [V]).
- Denoise / voice isolation: DeepFilterNet3 (MIT/Apache [V]) is the standout: ~10 MB, Rust-native, realtime on CPU, 48 kHz. Demucs v4 for music-vs-vocal stems (offline job).
- Scene detection: TransNetV2 (MIT [U]) on 48x27 downscaled frames; HSV-content/adaptive detector in pure Rust as zero-model fallback.
- Beat detection: beat_this (MIT code+weights [V]) for quality; Ellis DP onset tracker in Rust as fallback; avoid aubio (GPL), essentia (AGPL), madmom weights (NC-SA) [U].
- Auto-reframe tracking: YuNet/BlazeFace face detector + YOLOX/RT-DETRv2/D-FINE for objects (permissive [U]) + ByteTrack in Rust + spring-smoothed crop; avoid Ultralytics (AGPL) and SCRFD weights (NC) [U].

### Local provider implementing the same capability interface as cloud providers

Define capability traits once in `cutweave-ai-core`; both cloud adapters and the local plugin implement them, and the gateway routes by capability + policy (cloud / local / offline-fallback / privacy-mode = local-only).

```rust
#[async_trait] pub trait Stt: Send + Sync {
    async fn transcribe(&self, a: AudioRef, o: SttOpts, j: &JobCtx) -> Result<Transcript>; }
#[async_trait] pub trait Segmentation: Send + Sync {
    async fn matte_video(&self, v: VideoRef, prompt: SegPrompt /*Person|Points|Box*/, j: &JobCtx) -> Result<MaskTrack>; }
#[async_trait] pub trait AudioEnhance: Send + Sync { async fn denoise(&self, a: AudioRef, j: &JobCtx) -> Result<AudioBuf>; }
#[async_trait] pub trait Tts: Send + Sync { async fn synthesize(&self, t: &str, v: VoiceSpec, j: &JobCtx) -> Result<AudioBuf>; }
pub trait Provider { fn id(&self)->&str; fn caps(&self)->CapSet; fn locality(&self)->Locality /*Local|Cloud*/; fn cost(&self, r:&Request)->Option<Cost>; }
```
The local provider: (a) registers only the capabilities whose model packs are installed and whose hardware probe passes; (b) reports `Locality::Local` and zero cost so the router prefers it for cheap high-frequency calls when policy says so; (c) uses the same JobCtx (progress, cancellation token, VRAM budget) as the cloud path; (d) returns the same normalized schemas (Transcript, MaskTrack, ...) with provenance (model id, sha, license) stored in the project cache; (e) can run in-process (ort) or as a sidecar process for heavy models, hidden behind the trait. Cloud-only capabilities (video gen) simply have no local implementation; the router errors or prompts rather than silently falling back.

## Part B - Features

### 1. Speech-to-text with word timestamps

| Model | License | Size | Quality / speed | ONNX | Notes |
|---|---|---|---|---|---|
| Whisper large-v3-turbo | MIT [U] | ~1.6 GB fp16 (809M params); ~0.8 GB int8/q5 ggml ~ 550-900 MB | Best multilingual (99 langs) accuracy/speed trade-off; >10x realtime on mid GPU, ~1x or slower on CPU for large | Yes (onnx-community, sherpa-onnx exports [U]) | Word timestamps via cross-attention DTW (whisper.cpp supports) or via forced alignment. Hallucinations on silence -> use VAD (Silero, MIT [U]). **Recommended pick (GPU).** |
| Whisper small / base (ggml q5) | MIT [U] | 190-470 MB | Faster than realtime on CPU | Yes | **Fallback for CPU-only.** |
| distil-whisper large-v3 / v3.5 | MIT [U] | ~1.5 GB | English only | Yes | English-only users |
| faster-whisper / CTranslate2 | MIT [U] | same weights | Python/C++ lib; no Rust-first story, CUDA/CPU only | No (CT2 format) | Skip: use ort or whisper.cpp |
| NVIDIA Parakeet-tdt-0.6b-v3 | CC BY 4.0 [V] (attribution required, redistributable) | 0.6B params [V]; ONNX int8 ~640 MB (sherpa-onnx) [U] | 25 European languages [V], very fast, strong English WER; word/token timestamps native (TDT) | Yes (sherpa-onnx, onnx-asr) [U] | **Best pick for European languages if attribution OK; fastest on CPU.** Not for Asian languages. |
| Canary-1B-v2 / Qwen... | Canary-1B-flash CC BY 4.0, older Canary-1B CC BY-NC [U] | ~1-4 GB | Translation too | partial | Skip v1 |
| Moonshine tiny/base | MIT [V] | 27M/61M params [V] (~100-250 MB) | English only [V]; very low latency | Yes | Live preview captions on CPU; newer multilingual Moonshine variants exist but license varies (community license for non-English was noted) [U] |
| SenseVoice-Small | custom "model-license" [V - not read] | ~230M params (~900 MB fp32, ~230 MB int8) [U] | zh/yue/en/ja/ko [V] | Yes (sherpa-onnx) | Read license text before use; treat as non-redistributable until checked. |
| WhisperX (alignment wav2vec2) | BSD-2 code [U]; wav2vec2 aligner models mostly Apache/MIT per language [U] | 360 MB-1.2 GB per lang | Accurate word timing | Convertible | Python-heavy; instead use whisper.cpp DTW timestamps, or CTC aligner via ONNX (MMS-300M is CC BY-NC! avoid) [U] |

**Pick:** Whisper large-v3-turbo (ggml or ONNX) default; Parakeet-v3 as fast option for EU languages; whisper small/base + Silero VAD as CPU fallback. Streaming live captions: Moonshine (English) or sherpa-onnx streaming zipformer.

### 2. Diarization

| Option | License | Size | Notes |
|---|---|---|---|
| pyannote speaker-diarization-3.1 | MIT [V] but gated: must accept conditions and HF token, plus gated segmentation-3.0 [V]; collects user contact info [V] | ~35 MB total [U] | Best quality; gating is bad for onboarding. Pyannote community-1 pipeline (2025) license: [U] check. |
| sherpa-onnx diarization (pyannote-segmentation-3.0 ONNX + 3D-Speaker / NeMo / WeSpeaker embeddings) | sherpa Apache-2.0 [V]; segmentation model ONNX re-export retains pyannote MIT [U]; WeSpeaker Apache-2.0 [U]; 3D-Speaker Apache-2.0 [U] | segmentation ~6 MB, embedding 25-100 MB [U] | **Recommended**: ungated, Rust API, CPU realtime. Verify model-card license of each exported file. |

### 3. Segmentation / matting

| Model | License | Size | Notes |
|---|---|---|---|
| RobustVideoMatting (RVM) | **GPL-3.0 code [V]**; weights license unstated [V] | ~15 MB (MobileNetV3) [U] | Great temporal video matting, recurrent. GPL taints if code reused; ONNX export of weights likely derivative -> treat as GPL-risk. Avoid unless project is GPL-compatible (AGPL/GPL app is possible, but Cutweave wants permissive). |
| MODNet | Apache-2.0 [U] | ~26 MB | Portrait matting, image-based (no temporal) - add temporal smoothing; **recommended CPU/light fallback**. |
| BiRefNet (+ lite/ HR matting) | MIT [U] | 220 MB (swin_L), lite smaller | High quality still images, heavy for video; use for single-frame cutout. ONNX exists [U]. |
| RMBG-2.0 | **CC BY-NC 4.0 [V]**, commercial needs BRIA agreement [V]; 0.2B [V] | ~ 220 MB | DO NOT BUNDLE. RMBG-1.4 also NC [U]. |
| MediaPipe selfie segmentation | Apache-2.0 [U] | 0.2-0.5 MB (tflite) | Very fast, low quality edges; convert to ONNX (tflite2onnx) or use as lowest fallback. |
| PP-HumanSeg / PP-Matting (PaddleSeg) | Apache-2.0 [U] | 1-30 MB | Good permissive alternative; ONNX via Paddle2ONNX |
| SAM 2 / 2.1 (Meta) | Apache-2.0 (code + weights) [U] | tiny 39 MB, small 46, base+ 81, large 224 | Click-to-track with memory; ONNX exports exist (encoder/decoder/memory split) [U]; GPU needed for comfortable realtime-ish. **Pick for click-to-track.** SAM 3 (Nov 2025) uses SAM License (custom, permissive-ish) [U] - check. |
| EfficientTAM / EdgeTAM | Apache-2.0 [U] | ~ 20-40 MB | Faster on-device (EdgeTAM claims phone realtime) [U] - **fallback / CPU-friendly tracking**. |

**Pick:** background removal on video = MODNet (or PP-Matting) + temporal filter by default; BiRefNet for high-quality stills/export; SAM 2.1-tiny/small for click-to-track with EdgeTAM fallback. Evaluate RVM only if GPL acceptable.

### 4. TTS

| Model | License | Size | Notes |
|---|---|---|---|
| Kokoro-82M | Apache-2.0 [V] | 82M params [V] (~330 MB fp32, ~80 MB int8 ONNX [U]) | 8 languages, 54 voices (v1.0) [V]; fast on CPU (several x realtime) [U]; ONNX available; needs espeak-ng (GPL-3!) for phonemization in reference impl [U] -> use misaki/own G2P or bundle espeak-ng as separate dynamically-invoked process; **flag.** **Recommended pick.** |
| Piper (rhasspy) | MIT for old repo; new OHF-Voice/piper1-gpl is GPL-3 [U]; voices have individual licenses (many CC0/CC-BY, some NC) [U] | 15-100 MB | Also needs espeak-ng. Fallback; check each voice. Available via sherpa-onnx. |
| F5-TTS | Code MIT [U], **weights CC-BY-NC-4.0 [V]** | ~1.3 GB | DO NOT BUNDLE (trained on Emilia, NC). |
| XTTS-v2 | Coqui Public Model License (non-commercial) [U] | ~1.8 GB | DO NOT BUNDLE; Coqui defunct. |
| Chatterbox (Resemble) | MIT [V] | 500M params [V] (~2 GB) | Multilingual V3, 23 languages [V]; voice cloning; includes watermark (Perth) [U]; GPU recommended, heavy on CPU. **Optional "premium voice cloning" pack.** |
| Orpheus (Canopy) | Apache-2.0 [U] | 3B (~6 GB; quantized ~2 GB) | Llama-based, expressive, GPU-heavy; English mainly + multilingual research releases [U]. Plugin only. |
| Others [U] | Dia (Apache-2.0, 1.6B, English dialogue), Sesame CSM-1B (Apache-2.0), VibeVoice (MIT then restricted? verify), Qwen3-TTS (Apache-2.0? verify), Fish-Speech (weights CC-BY-NC-SA - avoid), CosyVoice 2 (Apache-2.0), Zonos (Apache-2.0), Higgs Audio | | Re-verify all; I did not check these. |

**Pick:** Kokoro-82M (+ Piper voices with verified permissive licenses as language fallback); Chatterbox as optional cloning pack.

### 5. Voice conversion

| Model | License | Notes |
|---|---|---|
| Seed-VC | **GPL-3.0 code, archived Nov 2025 [V]**; weights license not stated on repo [V] | Zero-shot VC, realtime-ish on GPU. Do not bundle code; weights unclear. |
| RVC (RVC-Project) | MIT code [U]; voice models are user-supplied, HuBERT/ContentVec base models MIT/Apache [U]; RMVPE pitch model MIT [U] | Needs per-voice trained models (user-supplied) - OK architecture for plugin; ONNX export supported [U]. **Plugin, not v1.** |
| kNN-VC / OpenVoice v2 (MIT [U]) / Chatterbox VC (MIT, [V] model is MIT) | | Chatterbox includes a VC mode [U] - simplest permissive path. |

**Pick:** v1 skip (or pitch/formant shift DSP via Rust: "chipmunk/robot" effects); plugin with RVC-ONNX or Chatterbox VC.

### 6. Denoise, enhancement, stems

| Model | License | Size | Notes |
|---|---|---|---|
| RNNoise | BSD-3 [U] | <1 MB | CPU trivial; mediocre; **CPU fallback**. |
| DeepFilterNet 2/3 | MIT or Apache-2.0 [V] | ~8-10 MB | Rust-native (tract) [V Rust core]; 48 kHz full-band; **Recommended pick**. ONNX exports available [U]. |
| sherpa-onnx speech enhancement (GTCRN etc.) | Apache-2.0 [V framework]; model licenses [U] | 0.5-10 MB | Alternate |
| Resemble-Enhance | MIT [U] | ~ 700 MB | Heavy, quality good for "voice enhance" - optional |
| Demucs v4 (htdemucs) | MIT [U] (repo archived by Meta Jan 2025; fork active) | htdemucs 80 MB; ft 4x ~ 320 MB | ONNX export feasible but STFT ops tricky; known ports in ONNX [U]. **Pick for stems**; ~realtime on GPU, 0.3-1x on CPU. |
| UVR / MDX-Net models | Code MIT [U]; individual model weights vary; many community checkpoints have unclear/NC terms [U] | 50-200 MB | Avoid unless each model's license verified. |
| BS-RoFormer / Mel-Band RoFormer (various) | repos MIT [U], weights vary | 200-900 MB | Better quality; verify weights |

### 7. Upscale / interpolation / beauty

| Task | Model | License | Size | Notes |
|---|---|---|---|---|
| Upscale | Real-ESRGAN (x4plus, animevideov3, general-x4v3) | BSD-3 [U] | 5-67 MB | Pick: realesr-general-x4v3 / animevideov3 (compact, ~5 MB) for video; ONNX widely available. Heavy per-frame; offline export-time feature on CPU. |
| | SPAN / Compact (OpenModelDB) | mostly permissive but per-model [U] | 1-10 MB | Fast alternates; check per-model license on OpenModelDB. |
| | SwinIR / HAT | Apache-2.0 [U] | 50-150 MB | Slow |
| Interpolation | RIFE (v4.x, practical-RIFE) | MIT [U] code; weights from authors - license ambiguous [U] | ~10-20 MB | Real-time on GPU for 1080p (ncnn-vulkan, ORT). Pick, verify weights terms. Frame-time ONNX needs custom grid_sample/warp (opset 16+). |
| | FILM (Google) | Apache-2.0 [U] | ~ 150 MB | Higher quality for large motion, slower. Fallback/quality mode. |
| | GIMM-VFI, EMA-VFI | check [U] | | |
| Beauty | MediaPipe Face Mesh/Landmarker (468/478 pts) | Apache-2.0 [U] | ~2-4 MB | Pick; convert tflite->ONNX or run via tract/tflite; shaders (bilateral/skin smoothing, warp for face slimming) in wgpu. |
| | BlazeFace | Apache-2.0 [U] | <1 MB | detector |
| | PFLD / 3DDFA, FaceXFormer | varied | | skip |

### 8. Auto-reframe + shot detection

| Model | License | Size | Notes |
|---|---|---|---|
| Ultralytics YOLOv5/8/11/12 | **AGPL-3.0 (code + weights)** [U] | 6-100 MB | TRAP: AGPL forces your app source open under AGPL; enterprise license otherwise. Cutweave is open source but wants permissive - avoid. |
| YOLOX | Apache-2.0 [U] | 9-100 MB | Permissive; ONNX first-class |
| RT-DETR (PaddleDetection) / RT-DETRv2 | Apache-2.0 [U] | 40-130 MB | Good quality, NMS-free |
| D-FINE / DEIM / RF-DETR (Roboflow, Apache-2.0 for most sizes) [U] | Apache-2.0 [U] | 5-100 MB | 2025 state of the art permissive; **Pick: RT-DETRv2-small or D-FINE-S / YOLOX-s**. Verify weights. |
| YOLO-NAS | custom non-commercial weights [U] | | avoid |
| BlazeFace / MediaPipe face detection | Apache-2.0 [U] | ~1 MB | Fast face pick; |
| SCRFD (insightface) | code MIT [U], **pretrained models non-commercial research only** [U] | 0.6-10 MB | Trap; retrain not feasible - use YuNet (OpenCV, MIT [U], 0.3 MB) instead. |
| Tracking | ByteTrack (MIT [U]) / OC-SORT (MIT [U]) - implement in Rust; Kalman + IoU is ~200 lines | | |
| Shot detection | TransNetV2 | MIT [U] | 30 MB | ONNX ports exist [U]; very accurate, GPU/CPU ok (~100+ fps GPU). **Pick.** |
| | PySceneDetect algorithms (ContentDetector HSV diff, AdaptiveDetector, threshold) | BSD-3 [U] | 0 | Reimplement in Rust (trivial); **CPU fallback** |

Auto-reframe design: detector at 5-10 fps on downscaled frame -> tracker -> smoothed crop path (Kalman / critically damped spring) with salient subject selection (largest face / active speaker from diarization).

### 9. Machine translation

| Model | License | Size | Quality / notes |
|---|---|---|---|
| NLLB-200 | CC-BY-NC 4.0 [U] | 600M-3.3B | DO NOT BUNDLE |
| MADLAD-400 3B/7B/10B | Apache-2.0 [V for 3B] | 3B: ~11.8 GB fp32, GGUF quant ~1.65 GB [V]; 7B Q4 ~4.5 GB [U] | 400+ languages, good quality for low-resource; T5 arch: llama.cpp supports T5 (GGUF) [V GGUF exists]. Heavier; **optional quality pack**. Note: license Apache but trained on web data (no restriction) [U]. |
| Opus-MT (Helsinki-NLP Marian) | CC-BY-4.0 mostly [U] (per-pair cards; some NC?) | 75-300 MB per pair (~ 75 MB int8 CT2/ONNX) | Per-language-pair models, fast on CPU (Marian). **Pick for default, common pairs;** many pairs; verify each card. Bergamot/Firefox Translations models (MPL-2.0 [U]) are an alternative with tiny ~20-40 MB models. |
| Small LLMs (Qwen3-4B Apache-2.0 [V], Gemma 3/3n custom terms [U], Llama 3.x custom [U]) | | 2-3 GB Q4 | Good for context-aware subtitle translation, SRT-aware prompts; slower. TranslateGemma (Gemma terms) [U]. |
| Whisper `translate` task | MIT [U] | - | to English only |

**Pick:** Opus-MT / Bergamot per pair (fast CPU) + Qwen3-4B (Apache-2.0) for pivoting/quality; MADLAD-400-3B GGUF as long-tail languages pack.

### 10. Highlights / shorts / silence & filler removal

- Silence removal: no ML; use VAD (Silero VAD, MIT [U]) + energy threshold. Filler words ("um/uh"): use ASR word timestamps; Whisper tends to omit fillers - use prompt trick (a prompt with fillers) or Parakeet/CrisperWhisper [license CC-BY-NC [U] - avoid].
- Highlights: transcript chunks -> LLM scoring (hook, completeness, sentiment) + audio energy/laughter + scene boundaries; output ranked segments in JSON (use llama.cpp grammar-constrained JSON).
- LLM license table:

| LLM | License | Notes |
|---|---|---|
| Qwen3 (0.6B-32B), Qwen3-4B-Instruct-2507 | Apache-2.0 [V for 4B-2507; 4.0B params] | **Pick**: 4B Q4_K_M ~2.5 GB; 1.7B / 0.6B for low-end. Qwen3 later versions (2.5/3.5 families?) - verify [U]. Some larger Qwen (e.g. 3B Qwen2.5) use Qwen Research license [U] - check size-by-size. |
| Phi-4-mini / Phi-3.5 | MIT [U] | 3.8B; strong reasoning, English-centric |
| Mistral 7B / Mistral Small 3 / Ministral | Apache-2.0 [U] (Ministral 3B/8B older: Mistral Research License - avoid) [U] | |
| Gemma 3 / 3n | Gemma Terms of Use: custom, use restrictions flow down to redistributors [U] | Avoid as default; can be user-downloaded optional |
| Llama 3.x | Llama Community License: custom, attribution, 700M MAU clause, acceptable use policy [U] | Not OSI open; avoid as default |
| SmolLM3 (Apache-2.0 [U]), OLMo 2/3 (Apache-2.0 [U]), IBM Granite (Apache-2.0 [U]) | | Alternatives |

### 11. Beat detection

| Option | License | Notes |
|---|---|---|
| beat_this (CPJKU, ISMIR 2024) | MIT code and weights [V] | Transformer, SOTA, beats + downbeats; ONNX not mentioned [V] - export feasible (check STFT/conv ops) [U]; ~ 20 MB [U]. **Pick** (GPU/CPU both fine offline). |
| madmom | BSD code, but model/data files CC BY-NC-SA [U] | TRAP for bundling models |
| aubio | GPL-3 [U] | TRAP (don't link) |
| essentia | AGPL-3 (commercial license available) [U] | TRAP |
| librosa-style onset+DP tempo (Ellis) / BTrack (GPL-3 [U]) | | Reimplement Ellis DP beat tracker in Rust (spectral flux + DP) - **CPU fallback**, no license issue. |

### 12. Generative image/video

| Model | License | Notes |
|---|---|---|
| SDXL base/turbo | CreativeML OpenRAIL++-M (use restrictions, redistributable) [U]; SDXL-Turbo: Stability non-commercial community license [U] | |
| SD 3.5 | Stability Community License (free <$1M revenue) [U] | custom |
| FLUX.1-schnell | Apache-2.0 [U] | 12B; ~ 6-12 GB quantized; heavy; **FLUX.1-dev is NC** [U] |
| Wan 2.1/2.2 | Apache-2.0 [U] | 1.3B model fits 8 GB; 5B/14B heavy |
| LTX-Video / LTX-2 | LTX-Video: OpenRAIL-style / custom (free <$10M revenue for newer) [U] | verify |
| HunyuanVideo | Tencent community license (excludes EU/UK/SK) [U] | avoid |
| CogVideoX | 2B Apache-2.0, 5B custom [U] | |

**Recommendation:** NOT in v1. Ship as plugin (separate process, separate download, separate license screen) with a ComfyUI / stable-diffusion.cpp / cloud-adapter backend. Reasons: VRAM (8-24 GB), minutes-per-clip, license heterogeneity, and it competes with the preview renderer.

### Summary pick table

| Feature | Pick | License | Size | Fallback |
|---|---|---|---|---|
| ASR | Whisper large-v3-turbo (+Silero VAD) | MIT [U] | ~0.8-1.6 GB | Whisper small/base q5; Parakeet v3 (CC BY 4.0 [V]) |
| Diarization | sherpa-onnx pyannote-seg-3.0 + WeSpeaker/3D-Speaker | MIT/Apache [U on exports] | ~50 MB | none (skip) |
| Matting | MODNet / PP-Matting; BiRefNet stills | Apache/MIT [U] | 25-220 MB | MediaPipe selfie |
| Click-to-track | SAM 2.1 tiny/small | Apache-2.0 [U] | 40-50 MB | EdgeTAM/EfficientTAM |
| TTS | Kokoro-82M | Apache-2.0 [V] | ~80-330 MB | Piper (per-voice check) |
| Voice conv | plugin (RVC / Chatterbox VC) | MIT [V for Chatterbox] | ~2 GB | - |
| Denoise | DeepFilterNet3 | MIT/Apache [V] | ~10 MB | RNNoise |
| Stems | Demucs v4 htdemucs | MIT [U] | 80-320 MB | - |
| Upscale | Real-ESRGAN compact | BSD-3 [U] | 5-67 MB | bicubic/Lanczos shader |
| Interp | RIFE v4.x (verify weights) | MIT [U] | 10-20 MB | FILM; blend |
| Beauty | MediaPipe face landmarker + shaders | Apache-2.0 [U] | ~4 MB | - |
| Reframe | D-FINE/RT-DETRv2/YOLOX + YuNet/BlazeFace + ByteTrack | Apache/MIT [U] | 1-100 MB | face-only BlazeFace |
| Shot detect | TransNetV2 | MIT [U] | ~30 MB | HSV content detector (Rust) |
| Translation | Opus-MT/Bergamot; Qwen3-4B; MADLAD-3B | CC-BY/MPL/Apache [U,V] | 40 MB-2.5 GB | cloud adapter |
| Highlights | Qwen3-4B-Instruct-2507 Q4 | Apache-2.0 [V] | ~2.5 GB | Qwen3-1.7B |
| Beats | beat_this | MIT [V] | ~20 MB [U] | Rust DP onset tracker |
| Gen image/video | plugin only | varied | GBs | - |

## Design notes: background AI jobs in Rust

- **Job system:** `AiJob { id, kind, inputs (asset hash + range), params, priority, gpu_mem_estimate, cancel_token }`. Persistent queue (SQLite) so jobs resume/re-queue after crash; results cached keyed by hash(media, model sha, params) in a project cache (captions, masks, beats are reusable and non-destructive).
- **Execution:** dedicated worker threads (not async executor) per resource class: `gpu`, `cpu`, `io`. Concurrency: 1 GPU job at a time by default; CPU jobs <= cores-2. Use tokio for orchestration + `spawn_blocking`/OS threads for inference. Heavy/unstable models (LLM, gen-AI, CUDA pack) run in a **sidecar process** over stdio/gRPC/local socket: protects the editor from driver crashes/OOM, enables hard kill = real cancellation, and allows different ORT versions.
- **Progress & cancellation:** chunked work (30 s audio windows, N-frame batches); each chunk reports `progress(f32, eta, partial_result)` through channel to UI; check `CancellationToken` between chunks (ORT run can be terminated via `RunOptions::terminate`  [U name]); stream partial results (captions appear as produced). Pause/resume at chunk boundary.
- **GPU contention with preview renderer:** preview has strict priority (frame deadline). Mechanisms: (1) VRAM budget manager - query adapter memory (DXGI/NVML/Metal), reserve renderer headroom (e.g. 1.5-2 GB + frame cache), refuse/queue AI jobs that exceed remainder, fall back to CPU or smaller model variant; (2) ORT EP `gpu_mem_limit` and arena shrink after jobs (`memory.enable_memory_arena_shrinkage`) [U]; unload sessions after idle timeout; (3) throttle: pause GPU jobs during playback/scrubbing (or yield per chunk), run at lower priority; on Windows use separate D3D queue priority / DirectML on a distinct queue [U], CUDA stream priorities [U]; (4) on integrated/Apple unified memory, cap AI to e.g. 40% of RAM; (5) renderer (wgpu) and ORT CUDA share device but not memory pool - zero-copy interop is hard, accept CPU round-trip for frames at AI resolution (downscale first, 256-1024 px).
- **Model sessions cache:** LRU of loaded sessions with per-model memory cost; warmup on first use (TensorRT/CoreML compilation can take 10s-minutes -> cache engines on disk, show "optimizing for your GPU" progress).
- **Telemetry-free benchmarking:** local one-time micro-benchmark to choose EP and model tier (e.g. "tiny / balanced / best").

## Optional BYO cloud adapter

Trait-based provider layer, off by default, keys stored in OS keychain (keyring crate), never in project files; explicit per-request disclosure of what data leaves the machine.

```rust
#[async_trait]
trait AiProvider: Send + Sync {
    fn id(&self) -> &str;
    fn capabilities(&self) -> Caps;                    // Transcribe, Translate, Tts, Llm, ImageGen, VideoGen...
    async fn transcribe(&self, a: AudioRef, o: AsrOpts, ctx: &JobCtx) -> Result<Transcript>; // word timestamps
    async fn translate(&self, segs: &[Segment], src: Lang, dst: Lang, ctx: &JobCtx) -> Result<Vec<Segment>>;
    async fn synthesize(&self, text: &str, v: VoiceSpec, ctx: &JobCtx) -> Result<AudioBuf>;
    async fn complete(&self, req: LlmRequest, ctx: &JobCtx) -> Result<LlmStream>;
    fn estimate_cost(&self, req: &Request) -> Option<Cost>;
}
```
Same job queue/cancel/progress, same output schemas as local models (so local/cloud are swappable), retries with backoff, rate limits, cost cap, and provider list as plugins (OpenAI-compatible endpoint covers many; ElevenLabs, Deepgram, AssemblyAI, DeepL, Anthropic, etc.).

## Open items to verify before implementation
Whisper/Silero/Demucs/MODNet/SAM2/TransNetV2/RIFE weights license text; Opus-MT per-pair licenses; SenseVoice model-license text; Piper voices; ort rc stability and ORT EP availability per OS; DirectML maintenance status; Qwen3 latest family; ONNX export feasibility for beat_this/Demucs/SAM2 memory modules; espeak-ng GPL implications for Kokoro/Piper.

## Sources
Verified (fetched this session):
- https://huggingface.co/briaai/RMBG-2.0
- https://huggingface.co/SWivid/F5-TTS
- https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3
- https://huggingface.co/hexgrad/Kokoro-82M
- https://huggingface.co/ResembleAI/chatterbox
- https://huggingface.co/google/madlad400-3b-mt
- https://huggingface.co/pyannote/speaker-diarization-3.1
- https://github.com/pykeio/ort
- https://github.com/CPJKU/beat_this
- https://github.com/PeterL1n/RobustVideoMatting
- https://github.com/Plachtaa/seed-vc
- https://github.com/k2-fsa/sherpa-onnx
- https://huggingface.co/UsefulSensors/moonshine
- https://huggingface.co/FunAudioLLM/SenseVoiceSmall
- https://github.com/Rikorose/DeepFilterNet
- https://huggingface.co/Qwen/Qwen3-4B-Instruct-2507
Unverified items [U] reference (to check): https://github.com/openai/whisper, https://github.com/ggml-org/whisper.cpp, https://github.com/ggml-org/llama.cpp, https://github.com/huggingface/candle, https://github.com/tracel-ai/burn, https://github.com/facebookresearch/sam2, https://github.com/xinntao/Real-ESRGAN, https://github.com/hzwer/ECCV2022-RIFE, https://github.com/ultralytics/ultralytics, https://github.com/soCzech/TransNetV2, https://github.com/Helsinki-NLP/Opus-MT, https://huggingface.co/black-forest-labs/FLUX.1-schnell, https://github.com/Wan-Video/Wan2.1, https://github.com/facebookresearch/demucs, https://github.com/aubio/aubio, https://github.com/CPJKU/madmom, https://github.com/MTG/essentia
