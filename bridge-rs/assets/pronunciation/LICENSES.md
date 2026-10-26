# Pronunciation data: sources and licences

Embedded in the bridge (`src/pronounce.rs`). `english.txt` and `common_*.txt` are rebuilt by
`scripts/pronunciation-assets.py` from the downloads below; the lexicons are hand-written.

| File | Source | Licence |
|---|---|---|
| `fr.json`, `de.json`, `es.json`, `en.json`, `nl.json` | written for BipAgents (the first French entries came from `numbers_fr`'s by-ear list) | MIT, as the bridge |
| `english.txt` | CMU Pronouncing Dictionary, <https://github.com/cmusphinx/cmudict> (`cmudict.dict`), words kept when among the 40 000 most frequent of FrequencyWords' English list | BSD-2-Clause style, (c) 1993-2015 Carnegie Mellon University; notice below |
| `common_fr.txt`, `common_de.txt`, `common_es.txt` | FrequencyWords 2018 by Hermit Dave, <https://github.com/hermitdave/FrequencyWords> (`content/2018/<lang>/<lang>_50k.txt`, built from OpenSubtitles 2018), filtered | content CC BY-SA 4.0 (code MIT); these filtered lists are shared under CC BY-SA 4.0 too |

The rank-filtered word lists and the English set only use the word forms (and the CMU pronunciations), not the
counts.

## CMUdict notice

Copyright (C) 1993-2015 Carnegie Mellon University. All rights reserved.

Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
   disclaimer. The contents of this file are deemed to be source code.
2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
   following disclaimer in the documentation and/or other materials provided with the distribution.

This work was supported in part by funding from the Defense Advanced Research Projects Agency, the Office of Naval
Research and the National Science Foundation of the United States of America, and by member companies of the
Carnegie Mellon Sphinx Speech Consortium. We acknowledge the contributions of many volunteers to the expansion and
improvement of this dictionary.

THIS SOFTWARE IS PROVIDED BY CARNEGIE MELLON UNIVERSITY ``AS IS'' AND ANY EXPRESSED OR IMPLIED WARRANTIES, INCLUDING,
BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN
NO EVENT SHALL CARNEGIE MELLON UNIVERSITY NOR ITS EMPLOYEES BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS
OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

## FrequencyWords

FrequencyWords (c) 2016 Hermit Dave. Content licensed under the Creative Commons Attribution-ShareAlike 4.0
International licence, <https://creativecommons.org/licenses/by-sa/4.0/>. Changes made here: lower-cased, letters-only
words kept, the first 15 000 words plus the later ones that are English words too, minus English words that leak into
the subtitles (see the build script).
