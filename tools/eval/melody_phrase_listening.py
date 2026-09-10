# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Create a standalone local four-condition listening report; upload nothing.

The optional scores manifest has schema_version=1 and variants mapping each of
baseline/pitch/rhythm/combined to aesthetics and clap {path, sha256} artifacts,
plus excerpt_sha256 mapping each case label to the scored excerpt's SHA256.
Without scores, objective cells remain blank. No human ratings are inferred.

    python tools/eval/melody_phrase_listening.py --manifest comparison/manifest.json \
        --scores scores.json --output comparison/listening.html
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from melody_ab import file_hash
from seed_listening import asset_url, number, script_json

CONDITIONS = ("baseline", "pitch", "rhythm", "combined")


def read(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def verified(value: dict) -> Path:
    path = Path(value["path"])
    if not path.is_absolute() or file_hash(path) != value["sha256"]:
        raise ValueError(f"Artifact path or hash differs: {path}")
    return path


def listening_data(
    manifest_path: Path, output: Path, scores_path: Path | None = None
) -> dict:
    """Validate all cohorts, local assets and optional score coverage before output."""
    experiment = read(manifest_path)
    if (
        experiment.get("schema_version") != 1
        or experiment.get("complete") is not True
        or set(experiment.get("variants", {})) != set(CONDITIONS)
    ):
        raise ValueError("A complete four-condition experiment is required")
    cases = experiment["cases"]
    labels = [f"{case['preset']}-s{case['seed']}" for case in cases]
    if not labels or len(labels) != len(set(labels)):
        raise ValueError("Cases must be nonempty and unique")
    manifests, scores = {}, {}
    score_spec = read(scores_path) if scores_path else None
    if score_spec is not None and (
        score_spec.get("schema_version") != 1
        or set(score_spec.get("variants", {})) != set(CONDITIONS)
    ):
        raise ValueError("Scores must cover exactly the four conditions")
    for name in CONDITIONS:
        path = verified(experiment["variants"][name])
        manifest = read(path)
        if (
            manifest.get("complete") is not True
            or manifest.get("condition") != name
            or manifest.get("cases") != cases
            or set(manifest.get("files", {})) != set(labels)
        ):
            raise ValueError(f"Condition cohort differs: {name}")
        manifests[name] = manifest
        if score_spec is not None:
            entry = score_spec["variants"][name]
            aesthetics, clap = (
                read(verified(entry[key])) for key in ("aesthetics", "clap")
            )
            if set(aesthetics) != set(labels) or set(clap.get("files", {})) != set(
                labels
            ):
                raise ValueError(f"Score cohort differs: {name}")
            expected_hashes = {
                label: row["excerpt"]["sha256"]
                for label, row in manifest["files"].items()
            }
            if entry.get("excerpt_sha256") != expected_hashes or any(
                clap["files"][label].get("sha256") != sha
                for label, sha in expected_hashes.items()
            ):
                raise ValueError(f"Scored audio differs from listening audio: {name}")
            if clap.get("preprocessing", {}).get("requested_segments") != 1:
                raise ValueError(f"CLAP must use one centered segment: {name}")
            if scores:
                baseline_clap = scores["baseline"][1]
                for key in ("prompts_sha256", "prompts", "model", "preprocessing"):
                    if clap.get(key) != baseline_clap.get(key):
                        raise ValueError(
                            f"CLAP {key} differs between conditions: {name}"
                        )
            scores[name] = (aesthetics, clap)
    groups = {}
    for case, label in zip(cases, labels):
        item = {**case, "label": label, "conditions": {}}
        baseline_excerpt = manifests["baseline"]["files"][label]["excerpt"]
        for name in CONDITIONS:
            row = manifests[name]["files"][label]
            if any(row.get(key) != value for key, value in case.items()):
                raise ValueError(f"Row metadata differs: {name}/{label}")
            data = {}
            for key in ("excerpt", "project", "wav"):
                verified(row[key])
                data[key] = asset_url(row[key], manifest_path.parent, output.parent)
            for key in (
                "duration_seconds",
                "start_tick",
                "end_tick",
                "start_frame",
                "end_frame",
                "bpm",
            ):
                if row["excerpt"][key] != baseline_excerpt[key]:
                    raise ValueError(f"Unequal listening excerpt: {name}/{label}")
            if row["excerpt"]["normalization"]["target_lufs"] != -23:
                raise ValueError(f"Unexpected listening level: {name}/{label}")
            data["duration"] = number(row["excerpt"]["duration_seconds"])
            data["scores"] = {"CE": None, "PQ": None, "CLAP": None}
            if name in scores:
                aesthetics, clap = scores[name]
                data["scores"] = {
                    "CE": number(aesthetics[label]["CE"]),
                    "PQ": number(aesthetics[label]["PQ"]),
                    "CLAP": number(
                        clap["files"][label]["aggregate"]["positive_cosine"]
                    ),
                }
            phrase = row["phrase"]
            data["note_count"] = phrase["note_count"]
            data["two_bar_motif_pairs"] = sum(
                pair["same_transposed_motif"] for pair in phrase["blocks"]["2"]["pairs"]
            )
            item["conditions"][name] = data
        groups.setdefault(case["preset"], []).append(item)
    return {
        "groups": [{"preset": key, "cases": value} for key, value in groups.items()]
    }


TEMPLATE = r"""<!doctype html>
<html lang="ja"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Auris — 音高とリズムを4条件で聴き比べる</title>
<style>
:root{color-scheme:light;font-family:system-ui,sans-serif;color:#27313a;background:#f4f5f6}*{box-sizing:border-box}body{max-width:1120px;margin:auto;padding:32px 22px 60px}h1{font-size:26px;margin:0 0 12px}h2{font-size:20px}p{line-height:1.7}.muted{font-size:14px;color:#52616f}.toolbar{display:flex;flex-wrap:wrap;gap:12px;align-items:center;margin:24px 0}select,button{font:inherit;padding:9px 14px;border:1px solid #a5aeb8;border-radius:8px;background:white;cursor:pointer}button:hover{background:#eaf0f5}:focus-visible{outline:3px solid #3b75b9;outline-offset:3px}button[aria-pressed=true]{background:#285681;color:white}.case{background:white;border:1px solid #dce1e6;border-radius:12px;margin:18px 0;padding:22px}.case-header{display:flex;align-items:center;flex-wrap:wrap;gap:12px;margin-bottom:18px}.case-header h3{margin:0;font-size:18px}.badge{font-size:12px;padding:4px 9px;background:#edf1f5;border-radius:6px}.conditions{display:grid;grid-template-columns:1fr 1fr;gap:24px}.condition{min-width:0}.condition h4{margin:0 0 8px;font-size:16px}.duration{margin:0 0 9px;font-size:13px;color:#52616f}audio{width:100%;min-width:0}.links{display:flex;flex-wrap:wrap;gap:12px;margin-top:10px;font-size:13px}a{color:#245b8b}.metrics{margin-top:20px;padding-top:14px;border-top:1px solid #dce1e6;overflow-x:auto}table{width:100%;font-size:13px;border-collapse:collapse}th,td{text-align:right;padding:7px 5px;border-bottom:1px solid #e5e8ec}th:first-child{text-align:left}.status{min-height:20px}[hidden]{display:none!important}@media(max-width:650px){body{padding:22px 14px}.case{padding:16px}.conditions{grid-template-columns:1fr;gap:22px}h1{font-size:23px}table{font-size:12px}th,td{padding:7px 3px}}
</style>
<h1>音高とリズムを、4つの条件で聴き比べる</h1>
<p class="muted">各曲で伴奏・ドラム・音色・ミキサー・演奏設定を固定しています。「基準」は前回の連続性改善後、「音高生成」は音の高さを作る処理、「リズムのみ」は発音位置と音価を変えています。「両方」で組み合わせた結果を確認できます。</p>
<p class="muted">「音高生成」は発音位置・音数・強さを維持します。同じ高さの音が続く場合は、重なりを解消する既存の処理によって一部の音価も変わります。この変化は音符ごとに記録し、それ以外の音価変更は比較に含めません。</p>
<p class="muted">試聴部分は同じ最初のサビ8小節です。全条件を−23 LUFSに揃え、両端に5 msのフェードを入れています。新しい条件を再生すると、ほかの音声は停止します。</p>
<div class="toolbar"><label for="genre">ジャンル</label><select id="genre"></select><button id="metrics" type="button" aria-pressed="false">測定値を表示</button><button id="stop" type="button">再生を止める</button></div>
<p id="status" class="muted status" role="status" aria-live="polite"></p><main id="groups"></main>
<p class="muted">「改善対象例」と「比較の基準例」は設計の参考にした曲です。「前回の検証用seed」は前回の実験で使った6曲です。今回は既知の比較例として、結果にかかわらず残しています。楽譜のリンクから各条件の主旋律を編集できます。</p>
<p class="muted">Audiobox CE・PQは楽しさと制作音質の予測値、CLAPは説明文との類似度です。モデルは同じ試聴抜粋を使い、CLAPは中央10秒を評価します。2小節の一致数は、4区間の6通りの組合せで音価・発音位置・移調を除いた音高列が一致した数です。数値だけでノリや覚えやすさの良し悪しは判断できません。</p>
<noscript>このローカル試聴ページにはJavaScriptが必要です。</noscript>
<script>
"use strict";
const DATA=/* DATA */null;
const names={baseline:"基準",pitch:"音高生成",rhythm:"リズムのみ",combined:"両方"};
const cohorts={diagnostic:"改善対象例",reference:"比較の基準例","held-out":"前回の検証用seed"};
const players=[],sections=[],tables=[];let revealed=false;
const el=(tag,text,cls)=>{const n=document.createElement(tag);if(text!==undefined)n.textContent=text;if(cls)n.className=cls;return n};
const pauseOthers=current=>players.forEach(audio=>{if(audio!==current)audio.pause()});
const genre=document.getElementById("genre"),status=document.getElementById("status");
const format=(value,digits)=>value===null?"—":value.toFixed(digits);
for(const group of DATA.groups){
  const option=el("option",group.preset);option.value=group.preset;genre.append(option);
  const section=el("section");section.dataset.preset=group.preset;section.append(el("h2",group.preset+" — "+group.cases.length+"曲"));
  for(const item of group.cases){
    const card=el("article",undefined,"case");card.dataset.label=item.label;
    const header=el("div",undefined,"case-header");header.append(el("h3","seed "+item.seed),el("span",cohorts[item.cohort],"badge"));
    const conditions=el("div",undefined,"conditions");
    for(const [name,title] of Object.entries(names)){
      const data=item.conditions[name],box=el("div",undefined,"condition"),audio=el("audio");audio.controls=true;audio.preload="none";audio.src=data.excerpt;audio.dataset.condition=name;
      audio.setAttribute("aria-label",group.preset+" seed "+item.seed+" "+title);
      audio.addEventListener("play",()=>{pauseOthers(audio);status.textContent=group.preset+" seed "+item.seed+"・"+title+"を再生中"});
      audio.addEventListener("error",()=>{status.textContent="音声を読み込めませんでした。ローカルの音声ファイルと試聴サーバーを確認してください。"});
      players.push(audio);box.append(el("h4",title),el("p",data.duration.toFixed(2)+"秒 / −23 LUFS","duration"),audio);
      const links=el("div",undefined,"links");for(const [text,url] of [["試聴WAV",data.excerpt],["編集可能な楽譜",data.project],["曲全体（音量未調整）",data.wav]]){const a=el("a",text);a.href=url;links.append(a)}box.append(links);conditions.append(box);
    }
    const metrics=el("div",undefined,"metrics");metrics.hidden=true;
    const table=el("table"),head=el("thead"),row=el("tr"),body=el("tbody");
    for(const text of ["条件","CE","PQ","CLAP","音数","2小節一致"]){const th=el("th",text);th.scope="col";row.append(th)}head.append(row);
    for(const [name,title] of Object.entries(names)){const data=item.conditions[name],tr=el("tr"),th=el("th",title);th.scope="row";tr.append(th);for(const value of [format(data.scores.CE,3),format(data.scores.PQ,3),format(data.scores.CLAP,4),data.note_count,data.two_bar_motif_pairs+" / 6"])tr.append(el("td",value));body.append(tr)}
    table.append(head,body);metrics.append(table);tables.push(metrics);card.append(header,conditions,metrics);section.append(card);
  }
  sections.push(section);document.getElementById("groups").append(section);
}
function showGenre(){pauseOthers(null);sections.forEach(section=>{section.hidden=section.dataset.preset!==genre.value});status.textContent=""}
genre.addEventListener("change",showGenre);showGenre();
document.getElementById("stop").addEventListener("click",()=>{pauseOthers(null);status.textContent="再生を停止しました。"});
document.getElementById("metrics").addEventListener("click",event=>{revealed=!revealed;tables.forEach(table=>{table.hidden=!revealed});event.currentTarget.setAttribute("aria-pressed",String(revealed));event.currentTarget.textContent=revealed?"測定値を隠す":"測定値を表示"});
</script></html>"""


def generate(
    manifest_path: Path, output: Path, scores_path: Path | None = None
) -> Path:
    """Write a new report only after its complete local data has been verified."""
    manifest_path, output = manifest_path.resolve(), output.resolve()
    if output.exists():
        raise ValueError(f"Refusing to overwrite: {output}")
    data = listening_data(manifest_path, output, scores_path)
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("x", encoding="utf-8", newline="\n") as stream:
        stream.write(TEMPLATE.replace("/* DATA */null", script_json(data)))
    return output


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--scores", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    print(generate(args.manifest, args.output, args.scores))


if __name__ == "__main__":
    main()
