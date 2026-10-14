# Drama 3 voices (Fish Audio Drama 3 on audio.cpp)

The Bips' voices served by [audio.cpp](https://github.com/JarodMica/audio.cpp) (Apache 2.0), branch `drama-3-native`
at `27e92a31737382c9564f0f991666ab9169ca8438`, with the local patches in `patches/` (speaker turns across chunks,
pauses, near-real-time streaming, prefix and reference caches…). Model: `Jmica/Drama-3-GGUF`
(`drama-3-experts-q4_k-backbone-q8_0.gguf`), its « tower » requantized to Q8_0 (`tower-tensors.tsv`). The voices
themselves are reference clips with their transcripts in `voices/drama/`.

Model license: Fish Audio Community License — personal and small commercial use; no hosted service for third parties;
products built on it say « Built with Fish Audio ».

Measured on a Strix Halo (Vulkan RADV): ~23 GiB of GPU memory, first audio 0.27 s once a voice is warm (~1 s the
first time), 1.2× real time. Set up by the installer (`installer/bipinstall/drama.py`, `install.sh drama`);
the bridge speaks to it as `drama:<id>` voices (`bridge/bipbridge/tts.py`).
