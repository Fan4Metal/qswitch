"""Builds the letter-trigram tables of the automatic switch (src/lang.rs).

For each language a table of costs, one byte per trigram of its alphabet
plus the word boundary: -log10 of the probability of the third letter after
the two before it, times 16, capped at 255. The probabilities come from a
word frequency list (hermitdave/FrequencyWords, OpenSubtitles 2018, CC
BY-SA 4.0), each word weighted by the logarithm of its count, with add-k
smoothing. The lists are downloaded into vendor/ngrams when missing; the
tables go to data/ngrams_<lang>.bin, which the program embeds.

    python tools/make_ngrams.py            # build the tables and print the calibration
    python tools/make_ngrams.py --check    # the calibration only (the tables as built)

The calibration types the most frequent words of each language in the
other layout of the QWERTY/ЙЦУКЕН pair and prints how the margin (the
cost read in the layout the word was typed in, minus the cost read in the
other one) is distributed for right and for wrong words, so the
thresholds in lang.rs can be chosen with their false-alarm rates known.
"""

import math
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VENDOR = ROOT / "vendor" / "ngrams"
DATA = ROOT / "data"
URL = "https://raw.githubusercontent.com/hermitdave/FrequencyWords/master/content/2018/{lang}/{lang}_50k.txt"

ALPHABETS = {
    "en": "abcdefghijklmnopqrstuvwxyz'",
    "ru": "абвгдежзийклмнопрстуфхцчшщъыьэюяё",
}
# Smoothing: pseudo-count of an unseen trigram.
K = 0.02
# A trigram never seen costs at least this (-log10): in a rare context the
# smoothing alone would make it cheap, while the lists are large enough
# for an unseen trigram to be one the language does not have.
FLOOR = 4.0
SCALE = 16.0

# The keys of the pair, for the calibration: the same key in each layout.
EN_KEYS = "qwertyuiop[]asdfghjkl;'zxcvbnm,."
RU_KEYS = "йцукенгшщзхъфывапролджэячсмитьбю"
EN_TO_RU = dict(zip(EN_KEYS, RU_KEYS))
RU_TO_EN = dict(zip(RU_KEYS, EN_KEYS))


def words_of(lang):
    path = VENDOR / f"{lang}_50k.txt"
    if not path.exists():
        VENDOR.mkdir(parents=True, exist_ok=True)
        url = URL.format(lang=lang)
        print(f"downloading {url}")
        urllib.request.urlretrieve(url, path)
    alphabet = set(ALPHABETS[lang])
    words = []
    for line in path.read_text(encoding="utf-8").splitlines():
        parts = line.split()
        if len(parts) != 2:
            continue
        word, count = parts[0].lower(), int(parts[1])
        if all(c in alphabet for c in word):
            words.append((word, count))
    return words


def build(lang, words):
    alphabet = ALPHABETS[lang]
    index = {c: i + 1 for i, c in enumerate(alphabet)}
    n = len(alphabet) + 1
    counts = [0.0] * (n * n * n)
    contexts = [0.0] * (n * n)
    for word, count in words:
        weight = math.log2(count + 1)
        seq = [0, 0] + [index[c] for c in word] + [0]
        for i in range(2, len(seq)):
            a, b, c = seq[i - 2], seq[i - 1], seq[i]
            counts[(a * n + b) * n + c] += weight
            contexts[a * n + b] += weight
    table = bytearray(n * n * n)
    for ab in range(n * n):
        total = contexts[ab] + K * n
        for c in range(n):
            p = (counts[ab * n + c] + K) / total
            cost = -math.log10(p)
            if counts[ab * n + c] == 0:
                cost = max(cost, FLOOR)
            table[ab * n + c] = min(255, round(cost * SCALE))
    return table


def cost(table, alphabet, word):
    """The mean cost per trigram of `word`, or None for a letter outside."""
    index = {c: i + 1 for i, c in enumerate(alphabet)}
    n = len(alphabet) + 1
    try:
        seq = [0, 0] + [index[c] for c in word] + [0]
    except KeyError:
        return None
    total = 0
    for i in range(2, len(seq)):
        total += table[(seq[i - 2] * n + seq[i - 1]) * n + seq[i]]
    return total / (len(seq) - 2) / SCALE


def percentiles(values, points=(1, 5, 25, 50, 75, 95, 99)):
    values = sorted(values)
    if not values:
        return "none"
    return "  ".join(f"p{p}={values[min(len(values) - 1, int(len(values) * p / 100))]:.2f}" for p in points)


def calibrate(tables, words):
    """Right words and wrong-layout words: the margin and the other cost."""
    for lang, other, to_other in (("en", "ru", EN_TO_RU), ("ru", "en", RU_TO_EN)):
        right = []  # (margin, cost of the other reading) for a word typed right
        wrong = []  # the same for the word typed in the other layout
        for word, _ in words[lang][:20000]:
            if len(word) < 3:
                continue
            own = cost(tables[lang], ALPHABETS[lang], word)
            mapped = "".join(to_other.get(c, c) for c in word)
            foreign = cost(tables[other], ALPHABETS[other], mapped)
            # Typed right: the current reading is `word` in `lang`, the other
            # reading is `mapped` in `other`.
            if own is not None:
                right.append((own - (foreign if foreign is not None else 99), foreign))
            # Typed in the other layout: the screen shows `mapped`, read in
            # `other`; the other reading is `word` in `lang`.
            if own is not None:
                wrong.append(((foreign if foreign is not None else 99) - own, own))
        print(f"\n{lang} words ({len(right)}), typed right: margin (current - other), lower is safer")
        print("  ", percentiles([m for m, _ in right]))
        print(f"{lang} words typed in the {other} layout: margin, higher is easier to catch")
        print("  ", percentiles([m for m, _ in wrong]))
        print(f"  their true reading's cost: ", percentiles([c for _, c in wrong]))
        for margin, plausible in ((1.0, 3.0), (1.5, 2.5), (2.0, 2.5), (2.5, 2.0), (3.0, 2.0)):
            false_alarms = sum(1 for m, c in right if m > margin and c is not None and c < plausible)
            caught = sum(1 for m, c in wrong if m > margin and c is not None and c < plausible)
            print(
                f"  margin>{margin} & other<{plausible}: false alarms {false_alarms / len(right):.4%}, "
                f"caught {caught / len(wrong):.2%}"
            )


def main():
    words = {lang: words_of(lang) for lang in ALPHABETS}
    if "--check" not in sys.argv:
        for lang in ALPHABETS:
            path = DATA / f"ngrams_{lang}.bin"
            table = build(lang, words[lang])
            DATA.mkdir(exist_ok=True)
            path.write_bytes(table)
            print(f"{path.relative_to(ROOT)}: {len(table)} bytes from {len(words[lang])} words")
    # The calibration on words the tables have not seen: every tenth word
    # is held out, and tables without them are built for it alone.
    held_out = {lang: [w for i, w in enumerate(words[lang]) if i % 10 == 0] for lang in ALPHABETS}
    rest = {lang: [w for i, w in enumerate(words[lang]) if i % 10 != 0] for lang in ALPHABETS}
    print()
    print("calibration on every tenth word, with tables built without them")
    calibrate({lang: build(lang, rest[lang]) for lang in ALPHABETS}, held_out)


if __name__ == "__main__":
    main()
