//! ONNX Runtime sessions shared by the singing backends.

use std::path::Path;

#[cfg(target_os = "macos")]
use ort::execution_providers::CoreML;
#[cfg(target_os = "windows")]
use ort::execution_providers::DirectML;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use ort::execution_providers::ExecutionProvider;
use ort::execution_providers::ExecutionProviderDispatch;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;

use crate::SingError;

/// Where a voice model's inference runs.
///
/// A preference about the machine, not the song: the same document renders through whichever
/// of these the settings name, and a frozen take keeps whatever it was sung with. The GPU is
/// reached through the platform's own provider — DirectML on Windows, Core ML on macOS — so
/// choosing it never installs anything.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, Hash,
)]
#[serde(rename_all = "lowercase")]
pub enum Acceleration {
    /// Sing on the GPU where the runtime offers one, and on the CPU where it does not.
    #[default]
    Auto,
    /// Insist on the GPU: loading fails visibly when it cannot be used, rather than
    /// falling back to a CPU render the person asked not to have.
    Gpu,
    /// Stay on the CPU.
    Cpu,
}

/// The GPU provider this platform reaches, with whether the linked runtime carries it.
///
/// The provider types are feature-gated for their target runtimes, so this selection is
/// target-gated as well. Other targets use the CPU provider.
fn gpu_provider() -> Option<(ExecutionProviderDispatch, bool)> {
    #[cfg(target_os = "windows")]
    {
        let provider = DirectML::default();
        let carried = provider.is_available().unwrap_or(false);
        Some((provider.build(), carried))
    }
    #[cfg(target_os = "macos")]
    {
        let provider = CoreML::default();
        let carried = provider.is_available().unwrap_or(false);
        Some((provider.build(), carried))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        None
    }
}

/// Builds the runtime session `acceleration`'s way, saying whether the GPU is in it.
pub(crate) fn open_session(
    path: &Path,
    acceleration: Acceleration,
) -> Result<(Session, bool), SingError> {
    open_session_with_optimization(path, acceleration, false)
}

pub(crate) fn open_session_with_optimization(
    path: &Path,
    acceleration: Acceleration,
    basic_optimization: bool,
) -> Result<(Session, bool), SingError> {
    let threads = std::thread::available_parallelism()
        .map(|cores| cores.get().saturating_sub(2).max(1))
        .unwrap_or(1);
    let refused = |error: ort::Error| SingError::Load {
        reason: error.to_string(),
    };
    let mut builder = Session::builder()
        .and_then(|builder| Ok(builder.with_intra_threads(threads)?))
        .and_then(|builder| {
            Ok(builder.with_optimization_level(if basic_optimization {
                GraphOptimizationLevel::Level1
            } else {
                GraphOptimizationLevel::Level3
            })?)
        })
        .map_err(refused)?;
    let gpu = match acceleration {
        Acceleration::Cpu => None,
        Acceleration::Auto => gpu_provider().filter(|(_, carried)| *carried),
        Acceleration::Gpu => Some(gpu_provider().ok_or(SingError::NoGpu)?),
    };
    let engaged = gpu.is_some();
    if let Some((provider, _)) = gpu {
        let provider = match acceleration {
            Acceleration::Gpu => provider.error_on_failure(),
            _ => provider,
        };
        builder = builder
            .with_execution_providers([provider])
            .map_err(|error| SingError::Load {
                reason: error.to_string(),
            })?;
        // DirectML cannot plan buffer reuse ahead of a run; the runtime wants memory
        // patterns off whenever it is in the session, and the other providers do not
        // miss them.
        builder = builder
            .with_memory_pattern(false)
            .map_err(|error| SingError::Load {
                reason: error.to_string(),
            })?;
    }
    match builder.commit_from_file(path) {
        Ok(session) => Ok((session, engaged)),
        Err(error) if engaged && acceleration == Acceleration::Auto => {
            log::warn!("the GPU refused the voice model ({error}); loading it on the CPU instead");
            open_session_with_optimization(path, Acceleration::Cpu, basic_optimization)
        }
        Err(error) => Err(refused(error)),
    }
}
