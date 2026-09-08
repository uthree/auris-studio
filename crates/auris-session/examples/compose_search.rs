//! Search a preset or song specification without opening an audio device.

use std::error::Error;
use std::path::PathBuf;

use auris_session::composition_search::{
    Candidate, CandidateFailure, CandidateOutcome, DensityTarget, Evaluation, ParameterBounds,
    PartDensity, SearchMethod, SearchSpace, SectionIntensity, SongSearchRequest,
    search_composition,
};
use auris_session::prelude::{Role, SongSpec, preset};
use serde_json::{Value, json};

const USAGE: &str = "usage: compose_search <preset-or-asong> <random|hill> <attempts> \
<search-seed> <composition-seed> <new-output-directory> [target-notes-per-bar]";

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    if !(6..=7).contains(&args.len()) {
        return Err(USAGE.into());
    }
    let mut base = match preset(&args[0]) {
        Some(preset) => preset.spec(),
        None => SongSpec::parse(&std::fs::read_to_string(&args[0])?)
            .map_err(|errors| format!("invalid song specification: {errors:?}"))?,
    };
    let algorithm = match args[1].as_str() {
        "random" => SearchMethod::Random,
        "hill" => SearchMethod::HillClimb,
        _ => return Err("algorithm must be random or hill".into()),
    };
    let attempt_budget = args[2].parse()?;
    let search_seed = args[3].parse()?;
    let composition_seed = args[4].parse()?;
    let output = PathBuf::from(&args[5]);
    if output.exists() {
        return Err("output directory already exists; choose a new directory".into());
    }
    let target_notes_per_bar = args.get(6).map_or(Ok(12.0), |value| value.parse())?;
    // Choose a writer whose density is used in at least one played section. Keep all authored
    // rhythm overrides and section tweaks; they remain fixed inputs to the search.
    let selected_part = base
        .parts
        .iter()
        .enumerate()
        .filter(|(_, part)| {
            part.rhythm.is_none()
                && !matches!(part.role, Role::Crash | Role::Riser)
                && base.form.iter().any(|name| {
                    base.sections.get(name).is_some_and(|section| {
                        (section.parts.is_empty() || section.parts.contains(&part.name))
                            && section
                                .tweaks
                                .get(&part.name)
                                .is_none_or(|tweak| tweak.density.is_none())
                    })
                })
        })
        .min_by_key(|(_, part)| part.role != Role::Melody)
        .map(|(index, _)| index);
    let bounds = || ParameterBounds {
        min: 0.0,
        max: 1.0,
        step: 0.1,
    };
    let part_density = selected_part.map(|index| {
        let part = &mut base.parts[index];
        part.density.get_or_insert(0.5);
        PartDensity {
            part: part.name.clone(),
            bounds: bounds(),
        }
    });
    let section = base.form.first().ok_or("song form is empty")?.clone();
    base.seed = composition_seed;
    let request = SongSearchRequest {
        base,
        space: SearchSpace {
            part_density,
            section_intensity: Some(SectionIntensity {
                section,
                bounds: bounds(),
            }),
        },
        attempt_budget,
        search_seed,
        composition_seed,
        algorithm,
        evaluator: DensityTarget {
            target_notes_per_bar,
        },
    };
    let result = search_composition(&request, || false)?;
    let history: Vec<_> = result
        .history
        .iter()
        .map(|attempt| {
            json!({
                "candidate": candidate_json(&attempt.candidate, &request.space),
                "outcome": outcome_json(&attempt.outcome),
            })
        })
        .collect();
    let best = result.best.as_ref().map(|best| {
        json!({
            "candidate": candidate_json(&best.candidate, &request.space),
            "evaluation": evaluation_json(&best.evaluation),
            "written_notes": best.score.note_count(),
            "summary": best.score.summary(),
            "spec_file": "best.asong",
        })
    });
    let report = json!({
        "package_version": env!("CARGO_PKG_VERSION"),
        "reproducibility": "Identical settings and seeds within the same build",
        "algorithm": args[1],
        "attempt_budget": request.attempt_budget,
        "search_seed": request.search_seed,
        "composition_seed": request.composition_seed,
        "target_notes_per_bar": request.evaluator.target_notes_per_bar,
        "base_spec_file": "base.asong",
        "space": {
            "part_density": request.space.part_density.as_ref().map(|parameter| json!({
                "part": parameter.part,
                "min": parameter.bounds.min,
                "max": parameter.bounds.max,
                "step": parameter.bounds.step,
            })),
            "section_intensity": request.space.section_intensity.as_ref().map(|parameter| json!({
                "section": parameter.section,
                "min": parameter.bounds.min,
                "max": parameter.bounds.max,
                "step": parameter.bounds.step,
            })),
        },
        "status": format!("{:?}", result.status()),
        "termination": format!("{:?}", result.termination),
        "failures": {
            "validation": result.failures.validation,
            "composition": result.failures.composition,
            "evaluation": result.failures.evaluation,
        },
        "best": best,
        "history": history,
    });
    // create_dir refuses an existing path even if another process created it during the run.
    std::fs::create_dir(&output)?;
    std::fs::write(output.join("base.asong"), request.base.to_toml())?;
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    if let Some(best) = &result.best {
        std::fs::write(output.join("best.asong"), best.candidate.params.to_toml())?;
        println!(
            "{} attempts; {:?}; best candidate {} with fitness {:.6}\n{}",
            result.history.len(),
            result.termination,
            best.candidate.id,
            best.evaluation.fitness,
            best.score.summary(),
        );
    } else {
        println!(
            "{} attempts; {:?}; no valid candidate; failures: {:?}",
            result.history.len(),
            result.termination,
            result.failures,
        );
    }
    println!("Report: {}", output.join("report.json").display());
    Ok(())
}

fn candidate_json(candidate: &Candidate<SongSpec>, space: &SearchSpace) -> Value {
    json!({
        "id": candidate.id,
        "composition_seed": candidate.seed,
        "part_density": space.part_density.as_ref().and_then(|parameter| {
            candidate.params.parts.iter().find(|part| part.name == parameter.part)
                .and_then(|part| part.density)
        }),
        "section_intensity": space.section_intensity.as_ref().and_then(|parameter| {
            candidate.params.sections.get(&parameter.section).map(|section| section.intensity)
        }),
    })
}

fn evaluation_json(evaluation: &Evaluation) -> Value {
    let metrics: Vec<_> = evaluation
        .metrics
        .iter()
        .map(|metric| json!({ "name": metric.name, "value": metric.value }))
        .collect();
    json!({ "fitness": evaluation.fitness, "metrics": metrics })
}

fn outcome_json(outcome: &CandidateOutcome) -> Value {
    match outcome {
        CandidateOutcome::Success(evaluation) => {
            json!({ "status": "success", "evaluation": evaluation_json(evaluation) })
        }
        CandidateOutcome::Failure(failure) => {
            let (stage, message) = match failure {
                CandidateFailure::Validation(error) => ("validation", error.to_string()),
                CandidateFailure::Composition(error) => ("composition", error.to_string()),
                CandidateFailure::Evaluation(error) => ("evaluation", error.to_string()),
            };
            json!({ "status": "failure", "stage": stage, "message": message })
        }
    }
}
