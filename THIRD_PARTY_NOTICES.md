# Third-party notices

Milkrice is MIT licensed (see `LICENSE`). It builds on the open models and libraries below. The website's [acknowledgements page](https://milkrice.yasithfernando.com/acknowledgements.html) is the user-facing version of this list.

## Speech and speaker models

Models are not bundled with Milkrice. Users download them on demand from their upstream sources and they stay under their authors' licenses.

| Model | From | License | Notes |
|---|---|---|---|
| Parakeet TDT 0.6B v2, v3 | NVIDIA | CC-BY-4.0 | ONNX int8 conversions by istupakov (changes: format conversion and quantization) |
| Canary 180M Flash, Canary 1B v2 | NVIDIA | CC-BY-4.0 | ONNX int8 conversions by istupakov (changes: format conversion and quantization) |
| Whisper Small, Medium, Large v3 Turbo | OpenAI | MIT | GGML conversions from whisper.cpp |
| Moonshine Base | Moonshine AI | MIT | |
| Moonshine v2 Medium (streaming) | Moonshine AI | MIT | mirrored unmodified at huggingface.co/milkrice/moonshine-streaming-medium-en |
| SenseVoice Small | FunAudioLLM | FunASR model license | |
| Cohere Transcribe | Cohere | see model card | |
| WeSpeaker ResNet34 (VoxCeleb) | WeSpeaker | see the WeSpeaker and sherpa-onnx repositories | speaker embeddings |

CC-BY-4.0: <https://creativecommons.org/licenses/by/4.0/>

## Libraries

Rust dependencies are listed with exact versions in `Cargo.lock`; each is under its own license (predominantly MIT and Apache-2.0). Notable ones: Tauri (MIT or Apache-2.0), transcribe-rs (MIT), ort / ONNX Runtime (MIT, Apache-2.0), whisper-rs and whisper.cpp (Unlicense, MIT), rusqlite and SQLite (MIT, public domain), cpal (Apache-2.0), aec3 (MIT or BSD-3-Clause; a Rust port of WebRTC's AEC3, with WebRTC's patent grant), screencapturekit-rs (MIT or Apache-2.0), reqwest and rustls (MIT or Apache-2.0).

The website uses the Newsreader typeface (SIL Open Font License 1.1).

A generated license report can be produced with `cargo install cargo-about && cargo about generate`.
