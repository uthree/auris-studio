# Datasets

What data exists for training a singing-voice foundation model, and what this
project decided to do with it. Licenses were verified against the official
distribution pages in August 2026; quotes and classifications below reflect
those pages, not folklore. The decisive constraint is not training — nearly
every license (and Japan's Copyright Act Art. 30-4) permits local training —
but **distributing the resulting weights**. Datasets are therefore grouped by
what they allow a *published* model to be.

## Tier A — weights may be published, commercially included

| Dataset | Amount | Content | License |
| --- | --- | --- | --- |
| [Namine Ritsu singing DB](https://www.canon-voice.com/voicebanks/) (official) | ~4.4 h | 1 female, 107 songs, 44.1 kHz, .lab + MusicXML | Custom: commercial use granted; **distributing an AI model is explicitly permitted** when the model reproduces Ritsu's voice; credit 「波音リツ」+「カノン」required |
| [SingStyle111](https://zenodo.org/records/10265401) | 12.8 h | 8 singers, EN/ZH/IT, 44.1 kHz, phoneme-aligned scores | CC BY 4.0 (Zenodo legal tag; the paper's "for research purposes" is aspirational, not a term) |
| [VocalSet](https://zenodo.org/records/1193957) | 10.1 h | 20 singers, vowels only | CC BY 4.0 |
| [PJS](https://sites.google.com/site/shinnosuketakamichi/research-topics/pjs_corpus) | 27 min | 1 male, 48 kHz, MusicXML + MIDI + .lab, phoneme-balanced | CC BY-SA 4.0 — the paper markets it as commercial-ready; whether SA attaches to trained weights is legally untested |
| [Dagstuhl ChoirSet](https://zenodo.org/records/3897182) | ~1-2 h raw | 5 bass singers, close mics, but 22.05 kHz | CC BY 4.0 |

The one catch in this tier: **Ritsu's terms (§7-2) prohibit publishing a model
that does *not* reproduce her voice** — that is, using the data inside a
generic base model meant to be fine-tuned toward other voices — unless the
software has an officially released Ritsu model. So Ritsu data goes into a
**single-speaker demo model** (the explicitly permitted shape, fine-tuning
prohibited on its model card), not into the published multi-speaker base,
short of negotiating with カノン.

## Tier B — non-commercial by default, with a real negotiation path

| Dataset | Amount | Path to publishing |
| --- | --- | --- |
| [JSUT-song](https://sites.google.com/site/shinnosuketakamichi/publication/jsut-song) | 25 min, 1 female, 48 kHz | UTokyo TLO commercial license ("we welcome your commercial use"). **Not** CC BY-SA — see `preprocessing.md` |
| [Tohoku Kiritan](https://zunko.jp/kiridev/login.php) / [Itako](https://zunko.jp/itadev/login.php) singing DBs | 57 min voiced / 50 songs, 96 kHz | SSS LLC; embedding a trained model in released software needs prior approval **even non-commercially** (§3-6); a per-song commercial rate (~¥1,000/song) is on record. No Zundamon singing DB exists |
| [JVS-MuSiC](https://sites.google.com/site/shinnosuketakamichi/research-topics/jvs_music) | 2.3 h, 100 singers (49M/51F), 24 kHz free tier | UTokyo TLO; 48 kHz/24-bit masters offered to commercial licensees |
| [jaCappella](https://tomohikonakamura.github.io/jaCappella_corpus/) | 35-50 min, 6 isolated parts incl. bass, 48 kHz | Paid commercial license, contacts published |
| [IdolSongsJp](https://huggingface.co/datasets/imprt/idol-songs-jp) | 15 songs, 18 singers, 48 kHz, per-singer dry stems | AIST; ML on vocal stems explicitly sanctioned, commercial use needs permission |
| [Ofuton-P singing DB](https://sites.google.com/view/oftn-utagoedb) | 46.5 min, 1 male, 96 kHz | Distributing a trained model requires prior inquiry |
| [Opencpop](https://github.com/wenet-e2e/opencpop) | 5.2 h, 1 female, Mandarin, 44.1 kHz | CC BY-NC-ND with an explicit "contact us for commercial" email path |

These are fine for **local, personal, research training** — the licenses gate
distribution, not the training run. That is what makes the
"users train their own voice models locally" distribution model work: recipes
can be published even where checkpoints cannot.

## Tier C — research only, no stated commercial path

[GTSinger](https://github.com/AaronZ345/GTSinger) (80.6 h, 9 languages,
20 singers, 48 kHz/24-bit, phoneme + technique labels, incl. 9.3 h of bass —
CC BY-NC-SA; by far the best *research* corpus),
[M4Singer](https://github.com/M4Singer/M4Singer) (29.7 h, CC BY-NC-SA),
OpenSinger (50 h but 24 kHz and no transcripts), PopCS,
ACE-Opencpop / ACE-KiSing (synthetic, CC BY-NC), CSD, NUS-48E/NHSS, DAMP
(non-commercial *and* no redistribution). The 1000+-hour corpora behind 2025-26
singing foundation models (SingNet, SoulX-Singer) were never released.

## Candidate auxiliary data

[Tsukuyomi-chan](https://tyc.rei-yumesaki.net/) — no sung-phrase database
exists, but the [corpus](https://tyc.rei-yumesaki.net/material/corpus/)
(100 + ~1,500 read utterances, 96 kHz) and the
[UTAU voicebank](https://tyc.rei-yumesaki.net/material/utau/) (mora samples,
multi-pitch) are covered by one of the most ML-friendly licenses surveyed:
synthesis software distribution is expressly permitted, commercial included,
with credit and content-restriction clauses
([terms](https://tyc.rei-yumesaki.net/material/utau/terms/)). Adopted as a
**candidate**: Japanese phonetic-coverage data for the published base model
(where lyric-bearing Japanese is otherwise 27 min of PJS), and a
cross-speaker-transfer experiment — can a speaker whose data never sings a
phrase borrow phrase dynamics from speakers who do. Speech-domain and
mora-fragment data cannot anchor singing dynamics by itself.

## Low pitch

No open corpus reaches below the ~87 Hz floor the current mix already has:
GTSinger's basses bottom at C#3 (138.6 Hz), open choir sets sit at E2-C3 with
sparse coverage, and the material that genuinely goes lower (barbershop
basses, oktavists) exists only as copyrighted commercial recordings. Dagstuhl
ChoirSet is the only CC BY candidate worth measuring (its 22.05 kHz sample
rate is a further problem for a 48 kHz model). Practically: **the 85 Hz floor
recorded in `training.md` stands until someone records bass singers.**

## The plan

1. **Published base model** (Hugging Face): Tier A minus Ritsu — VocalSet +
   SingStyle111 + Dagstuhl + PJS (+ Tsukuyomi-chan auxiliary). Multilingual
   phonetics, wide pitch range, thin but nonzero Japanese. Weight license
   CC BY-SA-compatible if PJS stays in.
2. **Published demo speaker**: single-speaker Namine Ritsu model — the shape
   her license explicitly blesses. Model card credits 波音リツ/カノン and
   prohibits fine-tuning to other voices.
3. **Local training by users**: recipes for Tier B/C corpora, which users may
   train under the personal/research grants; nothing NC-licensed ships as
   weights. Measured on this project's hardware, a 40k-step single-speaker
   run takes ~2 h (RTX PRO 4500) — small enough to make this a real workflow.
4. Negotiation backlog, in rough order of leverage: Tsukuyomi-chan singing
   recordings (no DB exists — ask), UTokyo TLO (JSUT-song/JVS-MuSiC), SSS
   (Kiritan per-song rate), カノン (Ritsu in the multi-speaker base).
