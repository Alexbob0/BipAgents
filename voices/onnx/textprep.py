"""The Bips' text, prepared as Pocket TTS 4.0.1 prepares it, without pocket or the `tokenizers` library: the reference
the apps' ports follow (Swift, Kotlin), checked against pocket itself (golden/<language>.json, test_textprep.py).

    chunks(text) -> [Chunk(text, ids, frames_after_eos, max_gen_len)]

1. The whole text is prepared (`prepare`): characters replaced (the language's table), spaces collapsed, semicolons
   made commas (fr, de), a capital first letter, a final punctuation mark.
2. It is cut on sentence ends (a period between two digits is a decimal), sentences longer than 50 tokens again on
   commas, semicolons and colons, then regrouped into chunks of at most 50 tokens.
3. Each chunk is prepared again and tokenized; it is spoken from the voice's state with `frames_after_eos` frames after
   the model says it's done (3 for four words or fewer, else 1; plus 2) and at most `max_gen_len` frames.

The tokenizer is Hugging Face tokenizers' Unigram model from the language's tokenizer.json: special tokens (<s>…) cut
out first; each other part gets ▁ in front, its spaces become ▁, it is split before each ▁, and each piece is cut by
the Viterbi algorithm over the vocabulary's scores; a character the vocabulary lacks goes as its UTF-8 bytes
(<0xNN>). Decoding drops special tokens, turns ▁ into spaces (every ▁ of the first token is dropped) and joins bytes
back into characters (an invalid run: one U+FFFD per byte).

Python's own string rules are the reference for what « whitespace », « uppercase » and « digit » mean."""
from __future__ import annotations

import json
import math
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List, Optional, Sequence, Tuple

SPACE = "▁"                      # ▁
UNK_PENALTY = 10.0
MAX_TOKENS = 50                       # pocket's MAX_TOKEN_PER_CHUNK
TOKENS_PER_SECOND = 3.0               # pocket's _TOKENS_PER_SECOND_ESTIMATE
PADDING_SECONDS = 2.0                 # pocket's _GEN_SECONDS_PADDING
FRAME_RATE = 12.5
TERMINAL = ".!?…"
WEAK = ",;:-–—"
CLOSERS = "\"'”’)]»"

# What each language's model was trained without (pocket_tts/config/<language>.yaml), and its other options.
QUOTES = {'"': '', '“': '', '”': '', '„': '', '«': '', '»': '', '’': "'", '‘': "'",
          '(': '', ')': '', '[': '', ']': ''}
# Pocket 4 (PR #387) adds dashes as commas (fr, es: –; de: – and —) and English's typographic apostrophes; the
# packages carry their own table (manifest `text`), so a phone's 3.3.0 package keeps the old one.
LANGUAGES: Dict[str, Dict] = {
    "french": {"replace": {**QUOTES, ":": ",", "–": ","}, "semicolons": True},
    "english": {"replace": {"’": "'", "‘": "'"}, "semicolons": False},
    "german": {"replace": {**QUOTES, "–": ",", "—": ","}, "semicolons": True},
    "spanish": {"replace": {**QUOTES, "¡": "", "¿": "", "–": ","}, "semicolons": False},
    "dutch": {"replace": {**QUOTES, "–": ","}, "semicolons": False},
}


class Tokenizer:
    """tokenizers' Unigram model with byte fallback, Prepend(▁) normalizer and Metaspace (always, split)."""

    def __init__(self, data: Dict):
        model = data["model"]
        if model.get("type") != "Unigram" or not model.get("byte_fallback"):
            raise ValueError("not a Unigram tokenizer with byte fallback")
        self.pieces: List[str] = [str(p) for p, _ in model["vocab"]]
        self.scores: List[float] = [float(s) for _, s in model["vocab"]]
        self.ids: Dict[str, int] = {}
        for i, piece in enumerate(self.pieces):
            self.ids.setdefault(piece, i)
        self.unk = int(model["unk_id"])
        self.min_score = min(self.scores)
        self.longest = max(len(p) for p in self.pieces)
        added = [(str(t["content"]), int(t["id"])) for t in data.get("added_tokens") or [] if t.get("special")]
        self.special = sorted(added, key=lambda item: -len(item[0]))
        self.special_ids = {i for _, i in added}

    @classmethod
    def load(cls, path: Path) -> "Tokenizer":
        return cls(json.loads(Path(path).read_text(encoding="utf-8")))

    # -- encoding -------------------------------------------------------------------------------------------------

    def _cut_special(self, text: str) -> List[Tuple[str, Optional[int]]]:
        """The text around special tokens (leftmost, then longest), each special token as its id."""
        parts: List[Tuple[str, Optional[int]]] = []
        start = i = 0
        while i < len(text):
            found = next(((s, sid) for s, sid in self.special if text.startswith(s, i)), None)
            if found is None:
                i += 1
                continue
            if start < i:
                parts.append((text[start:i], None))
            parts.append((found[0], found[1]))
            i += len(found[0])
            start = i
        if start < len(text):
            parts.append((text[start:], None))
        return parts

    @staticmethod
    def _words(text: str) -> List[str]:
        """▁ in front, spaces as ▁, split before each ▁ (it starts the next word)."""
        joined = SPACE + text.replace(" ", SPACE)
        words: List[str] = []
        current = ""
        for ch in joined:
            if ch == SPACE and current:
                words.append(current)
                current = ch
            else:
                current += ch
        if current:
            words.append(current)
        return words

    def _viterbi(self, word: str) -> List[str]:
        """The best cut of a word into pieces (ties: the first found), unknown characters fused."""
        size = len(word)
        best: List[Optional[Tuple[float, int, int]]] = [None] * (size + 1)   # (score, start, id)
        best[0] = (0.0, -1, -1)
        for start in range(size):
            here = best[start][0]
            single = False
            for length in range(1, min(self.longest, size - start) + 1):
                piece_id = self.ids.get(word[start:start + length])
                if piece_id is None:
                    continue
                score = here + self.scores[piece_id]
                end = start + length
                if best[end] is None or score > best[end][0]:
                    best[end] = (score, start, piece_id)
                if length == 1:
                    single = True
            if not single:
                score = here + self.min_score - UNK_PENALTY
                if best[start + 1] is None or score > best[start + 1][0]:
                    best[start + 1] = (score, start, self.unk)
        pieces: List[str] = []
        unknown: List[str] = []
        end = size
        while end > 0:
            _, start, piece_id = best[end]
            if piece_id == self.unk:
                unknown.append(word[start:end])
            else:
                if unknown:
                    pieces.append("".join(reversed(unknown)))
                    unknown = []
                pieces.append(word[start:end])
            end = start
        if unknown:
            pieces.append("".join(reversed(unknown)))
        return list(reversed(pieces))

    def _ids(self, piece: str) -> List[int]:
        found = self.ids.get(piece)
        if found is not None:
            return [found]
        as_bytes = [self.ids.get(f"<0x{b:02X}>") for b in piece.encode("utf-8")]
        return [int(i) for i in as_bytes] if all(i is not None for i in as_bytes) else [self.unk]

    def encode(self, text: str) -> List[int]:
        ids: List[int] = []
        for part, special in self._cut_special(text):
            if special is not None:
                ids.append(special)
                continue
            for word in self._words(part):
                for piece in self._viterbi(word):
                    ids += self._ids(piece)
        return ids

    # -- decoding -------------------------------------------------------------------------------------------------

    def decode(self, ids: Sequence[int]) -> str:
        tokens = [self.pieces[i] for i in ids if i not in self.special_ids]
        spaced = ["".join(("" if index == 0 else " ") if ch == SPACE else ch for ch in token)
                  for index, token in enumerate(tokens)]
        out: List[str] = []
        pending = bytearray()
        count = 0

        def flush() -> None:
            nonlocal pending, count
            if count:
                try:
                    out.append(bytes(pending).decode("utf-8"))
                except UnicodeDecodeError:
                    out.append("�" * count)
                pending, count = bytearray(), 0

        for token in spaced:
            match = re.fullmatch(r"<0x([0-9A-Fa-f]{2})>", token)
            if match:
                pending.append(int(match.group(1), 16))
                count += 1
            else:
                flush()
                out.append(token)
        flush()
        return "".join(out)


# -- the text --------------------------------------------------------------------------------------------------------

def _terminal(text: str) -> str:
    core = text.rstrip(CLOSERS + " ")
    closers = text[len(core):].strip()
    if not core or core[-1] in TERMINAL:
        return text
    if core[-1] in WEAK:
        return core.rstrip(WEAK + " ") + "." + closers
    return text + "."


def prepare(text: str, language: str) -> Tuple[str, int]:
    """A text as the model reads it, and how many frames to keep after it says it's done (before the +2)."""
    options = LANGUAGES[language]
    text = text.strip()
    if options["replace"]:
        text = " ".join(text.translate(str.maketrans(options["replace"])).split())
        text = re.sub(r"([.!?…])\s*[,;:]", r"\1", text)
    if text == "":
        raise ValueError("empty text")
    text = text.replace("\n", " ").replace("\r", " ").replace("  ", " ")
    if options["semicolons"]:
        text = text.replace(";", ",")
    after = 3 if len(text.split()) <= 4 else 1
    if not text[0].isupper():
        text = text[0].upper() + text[1:]
    return _terminal(text), after


def _boundaries(tokenizer: Tokenizer, ids: List[int], ends: Sequence[int], decimals: bool) -> List[int]:
    cuts = [0]
    after_end = False
    for index, token in enumerate(ids):
        if token in ends:
            after_end = True
            continue
        if after_end:
            if decimals:
                before = tokenizer.decode(ids[:index])
                rest = tokenizer.decode(ids[index:])
                if len(before) >= 2 and before[-1] == "." and before[-2].isdigit() and rest and rest[0].isdigit():
                    after_end = False
                    continue
            cuts.append(index)
        after_end = False
    cuts.append(len(ids))
    return cuts


def _segments(tokenizer: Tokenizer, ids: List[int], cuts: List[int]) -> List[Tuple[int, str]]:
    return [(cuts[i + 1] - cuts[i], tokenizer.decode(ids[cuts[i]:cuts[i + 1]])) for i in range(len(cuts) - 1)]


def sentences(tokenizer: Tokenizer, text: str, language: str, max_tokens: int = MAX_TOKENS) -> List[str]:
    """pocket's split_into_best_sentences: the prepared text cut into chunks of at most `max_tokens` tokens."""
    text = prepare(text, language)[0].strip()
    ids = tokenizer.encode(text)
    ends = tokenizer.encode(".!...?")[1:]
    segments = _segments(tokenizer, ids, _boundaries(tokenizer, ids, ends, decimals=True))
    weak = tokenizer.encode(",;:")[1:]
    refined: List[Tuple[int, str]] = []
    for count, sentence in segments:
        if count <= max_tokens:
            refined.append((count, sentence))
            continue
        sub_ids = tokenizer.encode(sentence.strip())
        parts = _segments(tokenizer, sub_ids, _boundaries(tokenizer, sub_ids, weak, decimals=False))
        refined += parts if len(parts) > 1 else [(count, sentence)]
    chunks: List[str] = []
    current, size = "", 0
    for count, sentence in refined:
        if current == "":
            current, size = sentence, count
        elif size + count > max_tokens:
            chunks.append(current.strip())
            current, size = sentence, count
        else:
            current += " " + sentence
            size += count
    if current != "":
        chunks.append(current.strip())
    return chunks


@dataclass(frozen=True)
class Chunk:
    text: str
    ids: List[int]
    frames_after_eos: int
    max_gen_len: int


def max_gen_len(tokens: int) -> int:
    return math.ceil((tokens / TOKENS_PER_SECOND + PADDING_SECONDS) * FRAME_RATE)


def chunks(tokenizer: Tokenizer, text: str, language: str) -> List[Chunk]:
    """What the model speaks, chunk by chunk (pocket's generate_audio_stream)."""
    out: List[Chunk] = []
    for sentence in sentences(tokenizer, text, language):
        prepared, after = prepare(sentence, language)
        ids = tokenizer.encode(prepared)
        out.append(Chunk(prepared, ids, after + 2, max_gen_len(len(ids))))
    return out
