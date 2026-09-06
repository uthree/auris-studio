"""A bounded, local OpenAI-compatible endpoint for Qwen2-Audio inference."""

from __future__ import annotations

import argparse
import base64
import binascii
import io
import json
import logging
import math
import threading
import time
import traceback
import uuid
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Protocol

import numpy as np
import soundfile as sf
from scipy.signal import resample_poly

DEFAULT_MODEL = "Qwen/Qwen2-Audio-7B-Instruct"
MUSIC_FLAMINGO_MODEL = "nvidia/music-flamingo-2601-hf"
SAMPLE_RATE = 16_000
MAX_AUDIO_BYTES = 25 * 1024 * 1024
MAX_TEXT_BYTES = 64 * 1024
MAX_BODY_BYTES = 4 * ((MAX_AUDIO_BYTES + 2) // 3) + 2 * MAX_TEXT_BYTES
MAX_AUDIO_SECONDS = 30
MAX_GENERATION_TOKENS = 512
LOGGER = logging.getLogger("auris_audio_review")
INPUT_FORMAT_NOTE = (
    "Audio input: mono, 16 kHz. Stereo placement cannot be assessed from this input."
)


class RequestError(Exception):
    """A failure that can be returned to an HTTP client without a traceback."""

    def __init__(self, message: str, status: int = 400, code: str = "invalid_request"):
        super().__init__(message)
        self.status = status
        self.code = code


@dataclass
class ReviewRequest:
    """Validated chat content and decoded audio, in matching encounter order."""

    messages: list[dict[str, Any]]
    audio: list[np.ndarray]
    max_tokens: int
    temperature: float


@dataclass
class ReviewResult:
    """The model's answer and measured generation counts, when available."""

    text: str
    prompt_tokens: int | None = None
    completion_tokens: int | None = None
    finish_reason: str = "stop"


class Backend(Protocol):
    """Inference boundary; protocol tests use a fake without importing Torch."""

    @property
    def loaded(self) -> bool: ...

    def generate(self, request: ReviewRequest) -> ReviewResult: ...


def decode_wav(data: str, remaining_bytes: int) -> tuple[np.ndarray, int]:
    """Decode an in-memory WAV to mono 16 kHz without altering its level."""
    if not isinstance(data, str) or not data:
        raise RequestError("input_audio.data must be nonempty standard base64 WAV data")
    if len(data) > 4 * ((remaining_bytes + 2) // 3):
        raise RequestError("Combined WAV data exceeds 25 MiB", 413, "audio_too_large")
    try:
        raw = base64.b64decode(data, validate=True)
    except (ValueError, binascii.Error) as exc:
        raise RequestError("input_audio.data is not valid standard base64") from exc
    if len(raw) > remaining_bytes:
        raise RequestError("Combined WAV data exceeds 25 MiB", 413, "audio_too_large")
    if len(raw) < 12 or raw[:4] != b"RIFF" or raw[8:12] != b"WAVE":
        raise RequestError("Only RIFF/WAVE input is supported")
    try:
        with sf.SoundFile(io.BytesIO(raw)) as source:
            rate, channels, frames = source.samplerate, source.channels, source.frames
            if not 8_000 <= rate <= 192_000 or not 1 <= channels <= 8:
                raise RequestError("WAV must have 1-8 channels and a sample rate of 8-192 kHz")
            if frames <= 0:
                raise RequestError("WAV contains no audio frames")
            if frames > rate * MAX_AUDIO_SECONDS:
                raise RequestError("Each WAV must be at most 30 seconds; select a shorter excerpt")
            samples = source.read(dtype="float32", always_2d=True)
    except (sf.LibsndfileError, ValueError, RuntimeError) as exc:
        raise RequestError("WAV data could not be decoded") from exc
    if not np.isfinite(samples).all():
        raise RequestError("WAV samples must be finite")
    mono = samples.mean(axis=1, dtype=np.float32)
    if rate != SAMPLE_RATE:
        divisor = math.gcd(rate, SAMPLE_RATE)
        mono = resample_poly(mono, SAMPLE_RATE // divisor, rate // divisor)
    if not np.isfinite(mono).all():
        raise RequestError("WAV level exceeds the supported floating-point range")
    return np.ascontiguousarray(mono, dtype=np.float32), len(raw)


def parse_request(payload: Any, model: str, *, independent_audio: bool = False) -> ReviewRequest:
    """Validate the supported Chat Completions subset without accessing files or URLs."""
    if not isinstance(payload, dict):
        raise RequestError("Request body must be a JSON object")
    if payload.get("model", model) != model:
        raise RequestError(f"This server serves only {model}", 404, "model_not_found")
    if payload.get("stream", False) is not False:
        raise RequestError("Streaming is not supported; set stream to false")
    if payload.get("tools") or payload.get("functions"):
        raise RequestError("This review endpoint does not execute tools")
    if payload.get("n", 1) != 1:
        raise RequestError("Only one completion is supported")
    tokens = payload.get("max_completion_tokens", payload.get("max_tokens", 256))
    if isinstance(tokens, bool) or not isinstance(tokens, int) or tokens <= 0:
        raise RequestError("max_tokens must be a positive integer")
    temperature = payload.get("temperature", 0.0)
    if (
        isinstance(temperature, bool)
        or not isinstance(temperature, (float, int))
        or not math.isfinite(temperature)
        or not 0 <= temperature <= 2
    ):
        raise RequestError("temperature must be a finite number from 0 to 2")
    source_messages = payload.get("messages")
    if not isinstance(source_messages, list) or not 1 <= len(source_messages) <= 16:
        raise RequestError("messages must contain 1-16 system or user messages")
    messages: list[dict[str, Any]] = []
    audio: list[np.ndarray] = []
    used_bytes = 0
    text_bytes = 0

    def accept_text(value: Any) -> str:
        nonlocal text_bytes
        if not isinstance(value, str):
            raise RequestError("Text content must be a string")
        try:
            text_bytes += len(value.encode("utf-8"))
        except UnicodeError as exc:
            raise RequestError("Text content must be valid UTF-8") from exc
        if text_bytes > MAX_TEXT_BYTES:
            raise RequestError("Combined text exceeds 64 KiB", 413, "text_too_large")
        return value

    for message in source_messages:
        if not isinstance(message, dict) or message.get("role") not in ("system", "user"):
            raise RequestError("Only system and user messages are supported")
        role = message["role"]
        content = message.get("content")
        if isinstance(content, str):
            messages.append({"role": role, "content": accept_text(content)})
            continue
        if not isinstance(content, list) or not 1 <= len(content) <= 32:
            raise RequestError("Message content must be text or 1-32 content parts")
        parts: list[dict[str, str]] = []
        for part in content:
            if not isinstance(part, dict):
                raise RequestError("Each content part must be an object")
            if part.get("type") == "text":
                parts.append({"type": "text", "text": accept_text(part.get("text"))})
            elif part.get("type") == "input_audio" and role == "user":
                if len(audio) >= 2:
                    raise RequestError("Provide one or two audio excerpts")
                encoded = part.get("input_audio")
                if not isinstance(encoded, dict) or encoded.get("format") != "wav":
                    raise RequestError("input_audio.format must be wav")
                samples, size = decode_wav(encoded.get("data"), MAX_AUDIO_BYTES - used_bytes)
                used_bytes += size
                audio.append(samples)
                # This is a template marker, never a path for a media loader.
                parts.append({"type": "audio"})
            else:
                raise RequestError("Only text and user input_audio WAV parts are supported")
        messages.append({"role": role, "content": parts})
    if not audio:
        raise RequestError("Provide one or two input_audio WAV excerpts")
    token_cap = MAX_GENERATION_TOKENS * (len(audio) if independent_audio else 1)
    return ReviewRequest(messages, audio, min(tokens, token_cap), float(temperature))


class QwenBackend:
    """Lazily load the official Transformers model; no remote Python code is trusted."""

    model_class_name = "Qwen2AudioForConditionalGeneration"

    def __init__(
        self,
        model: str = DEFAULT_MODEL,
        cache_dir: str | None = None,
        quantization: str = "none",
        device: str = "auto",
        max_memory_gib: float | None = None,
        revision: str | None = None,
    ):
        self.model_name = model
        self.cache_dir = cache_dir
        self.quantization = quantization
        self.device = device
        self.max_memory_gib = max_memory_gib
        self.revision = revision
        self.metadata: dict[str, Any] = {"requested_revision": revision}
        self._model = None
        self._processor = None
        self._gpu_devices: list[int] = []

    @property
    def loaded(self) -> bool:
        """Whether inference weights have finished loading."""
        return self._model is not None

    def _load(self) -> None:
        import torch
        import transformers
        from transformers import AutoProcessor, BitsAndBytesConfig

        gpu = self.device != "cpu" and torch.cuda.is_available()
        if self.device == "cuda" and not gpu:
            raise RuntimeError(
                "--device cuda requires a working CUDA or supported HIP PyTorch build"
            )
        dtype = torch.float32
        if gpu:
            dtype = torch.bfloat16 if torch.cuda.is_bf16_supported() else torch.float16
        options: dict[str, Any] = {
            "cache_dir": self.cache_dir,
            "revision": self.revision,
            "trust_remote_code": False,
            "dtype": dtype,
            "attn_implementation": "sdpa",
            "device_map": "auto" if gpu else {"": "cpu"},
        }
        if gpu and self.device == "cuda" and self.max_memory_gib is None:
            options["device_map"] = {"": "cuda:0"}
        if gpu and self.max_memory_gib is not None:
            from accelerate.utils import get_max_memory

            memory = get_max_memory()
            memory[0] = int(self.max_memory_gib * 1024**3)
            options["max_memory"] = memory
        if self.quantization != "none":
            if not gpu:
                raise RuntimeError(
                    "Quantization requires a working GPU and compatible bitsandbytes build; "
                    "use --quantization none for CPU"
                )
            options["quantization_config"] = BitsAndBytesConfig(
                load_in_4bit=self.quantization == "nf4",
                load_in_8bit=self.quantization == "int8",
                bnb_4bit_quant_type="nf4",
                bnb_4bit_compute_dtype=dtype,
                # Quantize the language model, retaining the audio path's precision.
                llm_int8_skip_modules=[
                    "model.audio_tower",
                    "model.multi_modal_projector",
                    "lm_head",
                ],
                llm_int8_enable_fp32_cpu_offload=True,
            )
        processor = AutoProcessor.from_pretrained(
            self.model_name,
            cache_dir=self.cache_dir,
            revision=self.revision,
            trust_remote_code=False,
        )
        model_class = getattr(transformers, self.model_class_name)
        model = model_class.from_pretrained(self.model_name, **options)
        model.eval()
        self._processor, self._model = processor, model
        if gpu:
            self._gpu_devices = sorted(
                {
                    parameter.device.index or 0
                    for parameter in model.parameters()
                    if parameter.device.type == "cuda"
                }
            )
        self.metadata.update(
            {
                "resolved_revision": getattr(model.config, "_commit_hash", None),
                "torch_version": torch.__version__,
                "transformers_version": transformers.__version__,
                "hip_version": getattr(torch.version, "hip", None),
                "device": self.device,
                "dtype": str(dtype),
                "quantization": self.quantization,
                "model_class": self.model_class_name,
            }
        )
        LOGGER.info("Loaded %s: %s", self.model_name, json.dumps(self.metadata))

    def generate(self, request: ReviewRequest) -> ReviewResult:
        """Generate a review from the decoded arrays, preserving audio and text order."""
        try:
            return self._generate(request)
        except Exception as exc:
            # Completed traceback frames otherwise retain failed-request GPU tensors.
            failure = exc
            seen: set[int] = set()
            while failure is not None and id(failure) not in seen:
                seen.add(id(failure))
                traceback.clear_frames(failure.__traceback__)
                failure = failure.__cause__ or failure.__context__
            raise
        finally:
            self._release_gpu_cache()

    def _release_gpu_cache(self) -> None:
        if not self.loaded or not self._gpu_devices:
            return
        import torch

        try:
            for device in self._gpu_devices:
                with torch.cuda.device(device):
                    torch.cuda.empty_cache()
        except Exception as exc:
            # Cleanup failure must not replace the model's answer or original failure.
            LOGGER.warning("Could not release unused GPU allocator cache: %s", exc)

    def memory_stats(self) -> list[dict[str, int]]:
        """Read current and lifetime peak Torch allocator bytes without loading a model."""
        if not self.loaded or not self._gpu_devices:
            return []
        import torch

        return [
            {
                "device": device,
                "allocated_bytes": torch.cuda.memory_allocated(device),
                "reserved_bytes": torch.cuda.memory_reserved(device),
                "peak_allocated_bytes": torch.cuda.max_memory_allocated(device),
                "peak_reserved_bytes": torch.cuda.max_memory_reserved(device),
            }
            for device in self._gpu_devices
        ]

    def _generate(self, request: ReviewRequest) -> ReviewResult:
        import torch

        if not self.loaded:
            self._load()
        model, processor = self._model, self._processor
        prompt = processor.apply_chat_template(
            request.messages, add_generation_prompt=True, tokenize=False
        )
        inputs = processor(
            text=prompt,
            audio=request.audio,
            sampling_rate=SAMPLE_RATE,
            return_tensors="pt",
            **self._processor_options(),
        )
        prompt_tokens = inputs["input_ids"].shape[1]
        text_config = model.config.text_config
        context_limit = getattr(text_config, "max_position_embeddings", 8192)
        if prompt_tokens + request.max_tokens > context_limit:
            raise RequestError(
                f"Audio and text require {prompt_tokens} input tokens; shorten the prompt "
                f"to leave {request.max_tokens} output tokens within {context_limit}"
            )
        inputs = inputs.to(model.device)
        inputs["input_features"] = inputs["input_features"].to(model.model.audio_tower.dtype)
        generation = self._generation_options(request)
        with torch.inference_mode():
            generated = model.generate(**inputs, **generation)
        output_ids = generated[:, prompt_tokens:]
        text = processor.batch_decode(
            output_ids, skip_special_tokens=True, clean_up_tokenization_spaces=False
        )[0].strip()
        if not text:
            raise RuntimeError("The audio model returned an empty review")
        completion_tokens = output_ids.shape[1]
        eos = model.generation_config.eos_token_id
        eos_ids = eos if isinstance(eos, list) else [eos]
        ended = completion_tokens > 0 and output_ids[0, -1].item() in eos_ids
        finish_reason = (
            "length" if completion_tokens >= request.max_tokens and not ended else "stop"
        )
        return ReviewResult(text, prompt_tokens, completion_tokens, finish_reason)

    def _generation_options(self, request: ReviewRequest) -> dict[str, Any]:
        options: dict[str, Any] = {
            "max_new_tokens": request.max_tokens,
            "do_sample": request.temperature > 0,
        }
        if request.temperature > 0:
            options["temperature"] = request.temperature
        return options

    def _processor_options(self) -> dict[str, Any]:
        return {"padding": True}


def independent_requests(request: ReviewRequest) -> list[ReviewRequest]:
    """Use an identical focus prompt for separate excerpts without comparison labels."""
    if len(request.audio) != 2:
        return [request]
    if request.max_tokens < 2:
        raise RequestError("Two independent observations require at least two output tokens")
    system_text: list[str] = []
    focus_text: list[str] = []
    for message in request.messages:
        content = message["content"]
        parts = (
            [content]
            if isinstance(content, str)
            else [part["text"] for part in content if part["type"] == "text"]
        )
        for text in parts:
            # Auris labels map the output ordinals; they are not evidence about the sound.
            if text.startswith("Audio excerpts in order:\n") and "\n\n" in text:
                text = text.split("\n\n", 1)[1]
            (system_text if message["role"] == "system" else focus_text).append(text)
    boundary = (
        "Only one audio excerpt is supplied for this independent observation. "
        "Describe audible properties of this excerpt alone. Do not compare recordings, "
        "infer a change, or describe another excerpt. Use the original request only to "
        "choose which audible properties to discuss; a comparative question cannot be "
        "answered from this single excerpt. State uncertainty rather than guessing."
    )
    messages = [
        {"role": "system", "content": "\n\n".join([*system_text, boundary])},
        {
            "role": "user",
            "content": [
                {
                    "type": "text",
                    "text": boundary + "\n\nOriginal focus:\n" + "\n\n".join(focus_text),
                },
                {"type": "audio"},
            ],
        },
    ]
    budgets = [(request.max_tokens + 1) // 2, request.max_tokens // 2]
    return [
        ReviewRequest(messages, [audio], budget, request.temperature)
        for audio, budget in zip(request.audio, budgets, strict=True)
    ]


class MusicFlamingoBackend(QwenBackend):
    """Opt-in native MusicFlamingo inference, with isolated observations for pairs."""

    model_class_name = "MusicFlamingoForConditionalGeneration"
    independent_audio = True

    def __init__(self, model: str = MUSIC_FLAMINGO_MODEL, **kwargs: Any):
        super().__init__(model=model, **kwargs)

    def _load(self) -> None:
        from transformers import AutoConfig

        config = AutoConfig.from_pretrained(
            self.model_name,
            cache_dir=self.cache_dir,
            revision=self.revision,
            trust_remote_code=False,
        )
        if config.model_type != "musicflamingo":
            raise RuntimeError("The music-flamingo backend requires a native musicflamingo model")
        super()._load()

    def _generation_options(self, request: ReviewRequest) -> dict[str, Any]:
        # Native v5 forwards the KV cache and drops audio features after prefill.
        return {**super()._generation_options(request), "use_cache": True}

    def _processor_options(self) -> dict[str, Any]:
        # The native encoder adds its full 1500-position table after convolution.
        return {"text_kwargs": {"padding": True}, "audio_kwargs": {"padding": "max_length"}}

    def _generate(self, request: ReviewRequest) -> ReviewResult:
        if len(request.audio) == 1:
            return super()._generate(request)
        observations = [
            super(MusicFlamingoBackend, self)._generate(single)
            for single in independent_requests(request)
        ]
        text = (
            "Independent single-excerpt observations; no direct A/B judgment was made.\n\n"
            + "\n\n".join(
                f"Excerpt {index}:\n{observation.text}"
                for index, observation in enumerate(observations, 1)
            )
        )
        prompt_tokens = (
            sum(item.prompt_tokens for item in observations)
            if all(item.prompt_tokens is not None for item in observations)
            else None
        )
        completion_tokens = (
            sum(item.completion_tokens for item in observations)
            if all(item.completion_tokens is not None for item in observations)
            else None
        )
        reason = (
            "length" if any(item.finish_reason == "length" for item in observations) else "stop"
        )
        return ReviewResult(text, prompt_tokens, completion_tokens, reason)


class ReviewService:
    """Serialize model access while keeping health checks and validation responsive."""

    def __init__(self, backend: Backend, model: str = DEFAULT_MODEL):
        self.backend = backend
        self.model = model
        self._inference_lock = threading.Lock()

    def complete(self, payload: Any) -> dict[str, Any]:
        """Return a Chat Completions response, or an actionable protocol error."""
        request = parse_request(
            payload,
            self.model,
            independent_audio=getattr(self.backend, "independent_audio", False),
        )
        if not self._inference_lock.acquire(blocking=False):
            raise RequestError(
                "The audio model is busy; retry after the current review", 503, "busy"
            )
        try:
            result = self.backend.generate(request)
        finally:
            self._inference_lock.release()
        LOGGER.info(
            "Review finished: audio_count=%d requested_output_tokens=%d "
            "effective_output_tokens=%d completion_tokens=%s finish_reason=%s",
            len(request.audio),
            payload.get("max_completion_tokens", payload.get("max_tokens", 256)),
            request.max_tokens,
            result.completion_tokens,
            result.finish_reason,
        )
        if not result.text.strip():
            raise RequestError("The audio model returned an empty review", 502, "empty_response")
        response: dict[str, Any] = {
            "id": "chatcmpl-" + uuid.uuid4().hex,
            "object": "chat.completion",
            "created": int(time.time()),
            "model": self.model,
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": INPUT_FORMAT_NOTE + "\n\n" + result.text,
                    },
                    "finish_reason": result.finish_reason,
                }
            ],
        }
        if result.prompt_tokens is not None and result.completion_tokens is not None:
            response["usage"] = {
                "prompt_tokens": result.prompt_tokens,
                "completion_tokens": result.completion_tokens,
                "total_tokens": result.prompt_tokens + result.completion_tokens,
            }
        return response


def make_server(host: str, port: int, service: ReviewService) -> ThreadingHTTPServer:
    """Build an HTTP server; its default caller binds only to loopback."""

    class Handler(BaseHTTPRequestHandler):
        def setup(self) -> None:
            super().setup()
            self.connection.settimeout(30)

        def reply(self, status: int, payload: dict[str, Any]) -> None:
            body = json.dumps(payload, ensure_ascii=False, allow_nan=False).encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)
            self.close_connection = True

        def error(self, exc: RequestError) -> None:
            self.reply(
                exc.status,
                {
                    "error": {
                        "message": str(exc),
                        "type": "invalid_request_error" if exc.status < 500 else "server_error",
                        "code": exc.code,
                    }
                },
            )

        def do_GET(self) -> None:
            if self.path == "/healthz":
                self.reply(
                    200,
                    {
                        "status": "ok",
                        "model": service.model,
                        "loaded": service.backend.loaded,
                        "runtime": getattr(service.backend, "metadata", {}),
                        "gpu_memory": getattr(service.backend, "memory_stats", lambda: [])(),
                    },
                )
            elif self.path == "/v1/models":
                self.reply(
                    200,
                    {
                        "object": "list",
                        "data": [
                            {
                                "id": service.model,
                                "object": "model",
                                "created": 0,
                                "owned_by": "local",
                            }
                        ],
                    },
                )
            else:
                self.error(RequestError("Unknown endpoint", 404, "not_found"))

        def do_POST(self) -> None:
            if self.path != "/v1/chat/completions":
                self.error(RequestError("Unknown endpoint", 404, "not_found"))
                return
            try:
                if self.headers.get("Transfer-Encoding"):
                    raise RequestError("Transfer-Encoding is not supported")
                if self.headers.get_content_type() != "application/json":
                    raise RequestError("Content-Type must be application/json", 415)
                try:
                    length = int(self.headers["Content-Length"])
                except (TypeError, ValueError) as exc:
                    raise RequestError("A valid Content-Length is required", 411) from exc
                if length < 1 or length > MAX_BODY_BYTES:
                    raise RequestError("Request body exceeds the supported size", 413)
                raw = self.rfile.read(length)
                if len(raw) != length:
                    raise RequestError("Request body ended before Content-Length")
                try:

                    def reject_constant(value: str) -> None:
                        raise ValueError(f"Invalid JSON constant: {value}")

                    payload = json.loads(raw, parse_constant=reject_constant)
                except (ValueError, UnicodeError) as exc:
                    raise RequestError("Request body is not valid JSON") from exc
                self.reply(200, service.complete(payload))
            except RequestError as exc:
                self.error(exc)
            except TimeoutError:
                self.error(RequestError("Request body read timed out", 408, "read_timeout"))
            except (BrokenPipeError, ConnectionResetError):
                LOGGER.info("Client disconnected")
            except Exception as exc:
                LOGGER.exception("Audio inference failed")
                self.error(RequestError(str(exc)[:1000], 500, "inference_failed"))

        def log_message(self, message: str, *args: Any) -> None:
            LOGGER.info(message, *args)

    server = ThreadingHTTPServer((host, port), Handler)
    server.daemon_threads = True
    return server


def main() -> None:
    """Start the local reviewer; weights are loaded only on the first audio request."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=11435)
    parser.add_argument(
        "--backend", choices=("qwen2-audio", "music-flamingo"), default="qwen2-audio"
    )
    parser.add_argument("--model")
    parser.add_argument("--cache-dir")
    parser.add_argument(
        "--revision", help="Hugging Face commit or revision for model and processor"
    )
    parser.add_argument("--quantization", choices=("none", "nf4", "int8"), default="none")
    parser.add_argument("--device", choices=("auto", "cpu", "cuda"), default="auto")
    parser.add_argument("--max-memory-gib", type=float, help="GPU 0 memory cap with CPU offload")
    args = parser.parse_args()
    if args.max_memory_gib is not None and (
        not math.isfinite(args.max_memory_gib) or args.max_memory_gib <= 0
    ):
        parser.error("--max-memory-gib must be a positive finite number")
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    backend_class = MusicFlamingoBackend if args.backend == "music-flamingo" else QwenBackend
    model = args.model or (
        MUSIC_FLAMINGO_MODEL if args.backend == "music-flamingo" else DEFAULT_MODEL
    )
    backend = backend_class(
        model=model,
        cache_dir=args.cache_dir,
        quantization=args.quantization,
        device=args.device,
        max_memory_gib=args.max_memory_gib,
        revision=args.revision,
    )
    server = make_server(args.host, args.port, ReviewService(backend, model))
    LOGGER.info("Listening at http://%s:%s (model loads on first request)", args.host, args.port)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
