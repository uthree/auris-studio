"""Protocol and audio tests; no downloaded weights or GPU are required."""

import base64
import contextlib
import http.client
import io
import json
import sys
import threading
import weakref
from types import SimpleNamespace

import numpy as np
import pytest
import soundfile as sf

import server


def wav_part(samples=None, rate=16_000, subtype="PCM_16"):
    if samples is None:
        samples = np.zeros(1600, dtype=np.float32)
    output = io.BytesIO()
    sf.write(output, samples, rate, format="WAV", subtype=subtype)
    return {
        "type": "input_audio",
        "input_audio": {
            "data": base64.b64encode(output.getvalue()).decode("ascii"),
            "format": "wav",
        },
    }


def payload(*parts):
    return {
        "model": server.DEFAULT_MODEL,
        "messages": [
            {"role": "system", "content": "Describe only what you hear."},
            {"role": "user", "content": list(parts) or [wav_part()]},
        ],
        "stream": False,
        "temperature": 0,
        "max_tokens": 2048,
    }


class FakeBackend:
    loaded = False

    def __init__(self):
        self.requests = []

    def generate(self, request):
        self.requests.append(request)
        self.loaded = True
        return server.ReviewResult("The supplied model answer.", 42, 8)


def test_stereo_resampling_preserves_level_and_audio_order():
    time = np.arange(48_000, dtype=np.float32) / 48_000
    tone = 0.5 * np.sin(2 * np.pi * 440 * time)
    # The second channel is half as loud: downmixing must retain that relationship.
    stereo = np.column_stack([tone, tone * 0.5])
    request = server.parse_request(
        payload(
            {"type": "text", "text": "A before:"},
            wav_part(stereo, 48_000),
            {"type": "text", "text": "B after:"},
            wav_part(stereo * 0.25, 48_000),
        ),
        server.DEFAULT_MODEL,
    )
    assert request.messages[1]["content"] == [
        {"type": "text", "text": "A before:"},
        {"type": "audio"},
        {"type": "text", "text": "B after:"},
        {"type": "audio"},
    ]
    before, after = request.audio
    assert before.dtype == np.float32
    assert before.shape == after.shape == (16_000,)
    assert np.sqrt(np.mean(before**2)) == pytest.approx(0.375 / np.sqrt(2), rel=0.005)
    assert np.sqrt(np.mean(after**2) / np.mean(before**2)) == pytest.approx(0.25, rel=0.005)
    assert request.max_tokens == 512


def test_default_generation_budget_and_input_labels():
    value = payload({"type": "text", "text": "音源 1"}, wav_part())
    del value["max_tokens"]
    request = server.parse_request(value, server.DEFAULT_MODEL)
    assert request.max_tokens == 256
    assert request.messages[0]["content"] == "Describe only what you hear."
    assert request.messages[1]["content"][0]["text"] == "音源 1"


@pytest.mark.parametrize(
    ("audio_count", "independent", "cap"),
    [(1, False, 512), (2, False, 512), (1, True, 512), (2, True, 1024)],
)
def test_output_caps_preserve_explicit_smaller_budgets_and_total_default(
    audio_count, independent, cap
):
    values = payload(*[wav_part() for _ in range(audio_count)])
    parsed = server.parse_request(values, server.DEFAULT_MODEL, independent_audio=independent)
    assert parsed.max_tokens == cap
    values["max_completion_tokens"] = 200
    assert (
        server.parse_request(values, server.DEFAULT_MODEL, independent_audio=independent).max_tokens
        == 200
    )
    del values["max_completion_tokens"]
    del values["max_tokens"]
    assert (
        server.parse_request(values, server.DEFAULT_MODEL, independent_audio=independent).max_tokens
        == 256
    )


def test_music_pair_service_allocates_512_each_and_logs_only_budget_metadata(monkeypatch, caplog):
    requests = []

    def observe(self, request):
        requests.append(request)
        return server.ReviewResult("PRIVATE_MODEL_TEXT", 5, 3)

    monkeypatch.setattr(server.QwenBackend, "_generate", observe)
    service = server.ReviewService(server.MusicFlamingoBackend(), server.MUSIC_FLAMINGO_MODEL)
    values = payload({"type": "text", "text": "PRIVATE_PROMPT"}, wav_part(), wav_part())
    values["model"] = server.MUSIC_FLAMINGO_MODEL
    caplog.set_level("INFO", logger="auris_audio_review")
    result = service.complete(values)
    assert [request.max_tokens for request in requests] == [512, 512]
    assert result["usage"]["completion_tokens"] == 6
    assert "audio_count=2 requested_output_tokens=2048 effective_output_tokens=1024" in caplog.text
    assert "completion_tokens=6 finish_reason=stop" in caplog.text
    assert "PRIVATE" not in caplog.text


@pytest.mark.parametrize(
    ("field", "value", "message"),
    [
        ("stream", True, "Streaming"),
        ("tools", [{"type": "function"}], "tools"),
        ("model", "another/model", "serves only"),
        ("max_tokens", True, "positive integer"),
        ("max_tokens", 0, "positive integer"),
        ("temperature", float("nan"), "finite"),
        ("messages", [], "1-16"),
    ],
)
def test_invalid_request_options(field, value, message):
    request = payload()
    request[field] = value
    with pytest.raises(server.RequestError, match=message):
        server.parse_request(request, server.DEFAULT_MODEL)


@pytest.mark.parametrize(
    "part",
    [
        {"type": "input_audio", "input_audio": {"data": "%%%", "format": "wav"}},
        {"type": "input_audio", "input_audio": {"data": "file.wav", "format": "mp3"}},
        {"type": "audio_url", "audio_url": "http://localhost/private.wav"},
        {"type": "input_audio", "input_audio": {"url": "file:///private.wav", "format": "wav"}},
    ],
)
def test_only_embedded_base64_wav_is_accepted(part):
    with pytest.raises(server.RequestError):
        server.parse_request(payload(part), server.DEFAULT_MODEL)


def test_audio_count_and_combined_byte_limits(monkeypatch):
    with pytest.raises(server.RequestError, match="one or two"):
        server.parse_request(payload(wav_part(), wav_part(), wav_part()), server.DEFAULT_MODEL)
    with pytest.raises(server.RequestError, match="one or two"):
        server.parse_request(payload({"type": "text", "text": "No audio"}), server.DEFAULT_MODEL)
    part = wav_part()
    size = len(base64.b64decode(part["input_audio"]["data"]))
    monkeypatch.setattr(server, "MAX_AUDIO_BYTES", size * 2 - 1)
    with pytest.raises(server.RequestError) as failure:
        server.parse_request(payload(part, part), server.DEFAULT_MODEL)
    assert failure.value.status == 413


def test_rejects_long_empty_and_nonfinite_wavs():
    for samples in (np.zeros(16_000 * 31), np.zeros(0), np.array([np.nan, np.inf])):
        with pytest.raises(server.RequestError):
            server.parse_request(payload(wav_part(samples, subtype="FLOAT")), server.DEFAULT_MODEL)


def test_service_preserves_model_text_usage_and_refusal():
    backend = FakeBackend()
    service = server.ReviewService(backend)
    result = service.complete(payload())
    assert result["choices"][0]["message"]["content"] == (
        server.INPUT_FORMAT_NOTE + "\n\nThe supplied model answer."
    )
    assert result["usage"] == {"prompt_tokens": 42, "completion_tokens": 8, "total_tokens": 50}
    assert len(backend.requests[0].audio) == 1
    backend.generate = lambda _: server.ReviewResult("I cannot hear this audio.")
    assert service.complete(payload())["choices"][0]["message"]["content"] == (
        server.INPUT_FORMAT_NOTE + "\n\nI cannot hear this audio."
    )
    backend.generate = lambda _: server.ReviewResult("An incomplete", 42, 512, "length")
    assert service.complete(payload())["choices"][0]["finish_reason"] == "length"


def test_service_serializes_inference_and_releases_lock_after_failure():
    service = server.ReviewService(FakeBackend())
    with service._inference_lock, pytest.raises(server.RequestError) as failure:
        service.complete(payload())
    assert failure.value.status == 503

    def fail(_):
        raise RuntimeError("model unavailable")

    service.backend.generate = fail
    with pytest.raises(RuntimeError, match="model unavailable"):
        service.complete(payload())
    assert service._inference_lock.acquire(blocking=False)
    service._inference_lock.release()


@pytest.fixture
def endpoint():
    backend = FakeBackend()
    http_server = server.make_server("127.0.0.1", 0, server.ReviewService(backend))
    thread = threading.Thread(target=http_server.serve_forever, daemon=True)
    thread.start()
    yield http_server.server_address, backend
    http_server.shutdown()
    http_server.server_close()
    thread.join(timeout=2)


def http_json(endpoint, method, path, body=None, headers=None):
    address, _ = endpoint
    connection = http.client.HTTPConnection(*address, timeout=5)
    connection.request(method, path, body, headers or {})
    response = connection.getresponse()
    result = response.status, json.loads(response.read())
    connection.close()
    return result


def test_real_http_round_trip_decodes_audio_and_reports_health(endpoint):
    assert http_json(endpoint, "GET", "/healthz")[1]["loaded"] is False
    assert http_json(endpoint, "GET", "/v1/models")[1]["data"][0]["id"] == server.DEFAULT_MODEL
    status, result = http_json(
        endpoint,
        "POST",
        "/v1/chat/completions",
        json.dumps(payload()),
        {"Content-Type": "application/json"},
    )
    assert status == 200
    assert result["choices"][0]["finish_reason"] == "stop"
    assert result["choices"][0]["message"]["content"] == (
        server.INPUT_FORMAT_NOTE + "\n\nThe supplied model answer."
    )
    assert endpoint[1].requests[0].audio[0].shape == (1600,)
    assert http_json(endpoint, "GET", "/healthz")[1]["loaded"] is True


def test_http_input_format_note_preserves_refusal_and_pair_answer(endpoint):
    for answer in (
        "I cannot hear this audio.",
        "Independent single-excerpt observations.\nExcerpt 1: left.\nExcerpt 2: right.",
    ):
        endpoint[1].generate = lambda _, text=answer: server.ReviewResult(text)
        status, result = http_json(
            endpoint,
            "POST",
            "/v1/chat/completions",
            json.dumps(payload()),
            {"Content-Type": "application/json"},
        )
        assert status == 200
        assert result["choices"][0]["message"]["content"] == (
            server.INPUT_FORMAT_NOTE + "\n\n" + answer
        )


def test_http_errors_are_json_and_do_not_run_the_model(endpoint):
    for body, expected in (
        ("{broken", 400),
        (json.dumps({"stream": True}), 400),
        ('{"temperature":NaN}', 400),
    ):
        status, result = http_json(
            endpoint,
            "POST",
            "/v1/chat/completions",
            body,
            {"Content-Type": "application/json"},
        )
        assert status == expected
        assert result["error"]["message"]
    assert endpoint[1].requests == []
    assert http_json(endpoint, "GET", "/missing")[0] == 404


def test_qwen_adapter_passes_decoded_audio_with_current_processor_keyword(monkeypatch):
    monkeypatch.setitem(
        sys.modules, "torch", SimpleNamespace(inference_mode=contextlib.nullcontext)
    )
    observed = {}

    class Feature:
        def to(self, dtype):
            observed["audio_dtype"] = dtype
            return self

    class Inputs(dict):
        def to(self, device):
            observed["device"] = device
            return self

    class Processor:
        def apply_chat_template(self, messages, **kwargs):
            observed["messages"] = messages
            return "rendered audio markers"

        def __call__(self, **kwargs):
            observed.update(kwargs)
            return Inputs(input_ids=np.zeros((1, 5)), input_features=Feature())

        def batch_decode(self, output, **kwargs):
            assert output.tolist() == [[9, 2]]
            return ["An actual model answer."]

    class Model:
        device = "test_gpu"
        model = SimpleNamespace(audio_tower=SimpleNamespace(dtype="audio_precision"))
        config = SimpleNamespace(text_config=SimpleNamespace(max_position_embeddings=8192))
        generation_config = SimpleNamespace(eos_token_id=[2])

        def generate(self, **kwargs):
            assert kwargs["do_sample"] is False
            return np.array([[0, 0, 0, 0, 0, 9, 2]])

    backend = server.QwenBackend()
    backend._processor, backend._model = Processor(), Model()
    request = server.parse_request(payload(), server.DEFAULT_MODEL)
    result = backend.generate(request)
    assert observed["audio"] is request.audio
    assert observed["sampling_rate"] == 16_000
    assert observed["messages"] is request.messages
    assert observed["audio_dtype"] == "audio_precision"
    assert result == server.ReviewResult("An actual model answer.", 5, 2, "stop")

    backend._model.config.text_config.max_position_embeddings = 10
    with pytest.raises(server.RequestError, match="shorten"):
        backend.generate(request)


def test_loader_pins_both_components_and_records_runtime_without_gpu(monkeypatch):
    calls = {}
    fake_torch = SimpleNamespace(
        cuda=SimpleNamespace(is_available=lambda: False),
        float32="float32",
        __version__="test-torch",
        version=SimpleNamespace(hip=None),
    )

    def load_processor(name, **kwargs):
        calls["processor"] = (name, kwargs)
        return object()

    def load_model(name, **kwargs):
        calls["model"] = (name, kwargs)
        return SimpleNamespace(eval=lambda: None, config=SimpleNamespace(_commit_hash="pinned"))

    fake_transformers = SimpleNamespace(
        AutoProcessor=SimpleNamespace(from_pretrained=load_processor),
        Qwen2AudioForConditionalGeneration=SimpleNamespace(from_pretrained=load_model),
        BitsAndBytesConfig=object,
        __version__="test-transformers",
    )
    monkeypatch.setitem(sys.modules, "torch", fake_torch)
    monkeypatch.setitem(sys.modules, "transformers", fake_transformers)
    backend = server.QwenBackend(device="cpu", revision="pinned")
    backend._load()
    for _, kwargs in calls.values():
        assert kwargs["revision"] == "pinned"
        assert kwargs["trust_remote_code"] is False
    assert calls["model"][1]["device_map"] == {"": "cpu"}
    assert calls["model"][1]["dtype"] == "float32"
    assert backend.metadata["resolved_revision"] == "pinned"
    assert backend.metadata["torch_version"] == "test-torch"
    assert backend.loaded


@pytest.mark.parametrize("fail", [False, True])
def test_gpu_cleanup_runs_after_request_locals_are_released(monkeypatch, fail):
    workspace = []
    released = []

    class TemporaryTensor:
        pass

    def infer(_):
        temporary = TemporaryTensor()
        workspace.append(weakref.ref(temporary))
        if fail:
            raise RuntimeError("inference failed")
        return server.ReviewResult("Finished")

    def empty_cache():
        assert workspace[0]() is None
        released.append(True)

    monkeypatch.setitem(
        sys.modules,
        "torch",
        SimpleNamespace(
            cuda=SimpleNamespace(
                device=contextlib.nullcontext,
                empty_cache=empty_cache,
                memory_allocated=lambda _: 100,
                memory_reserved=lambda _: 120,
                max_memory_allocated=lambda _: 150,
                max_memory_reserved=lambda _: 200,
            )
        ),
    )
    backend = server.QwenBackend()
    retained_weights = object()
    backend._model = retained_weights
    backend._gpu_devices = [0]
    backend._generate = infer
    if fail:
        with pytest.raises(RuntimeError, match="inference failed"):
            backend.generate(None)
    else:
        assert backend.generate(None).text == "Finished"
    assert released == [True]
    assert backend._model is retained_weights
    assert backend.memory_stats() == [
        {
            "device": 0,
            "allocated_bytes": 100,
            "reserved_bytes": 120,
            "peak_allocated_bytes": 150,
            "peak_reserved_bytes": 200,
        }
    ]


def test_cpu_cleanup_does_not_access_torch_cuda(monkeypatch):
    monkeypatch.setitem(sys.modules, "torch", SimpleNamespace())
    backend = server.QwenBackend(device="cpu")
    backend._model = object()
    backend._generate = lambda _: server.ReviewResult("CPU answer")
    assert backend.generate(None).text == "CPU answer"
    assert backend.memory_stats() == []


def test_music_flamingo_pair_uses_isolated_identical_prompts_in_original_order(monkeypatch):
    values = payload(
        {
            "type": "text",
            "text": (
                "Audio excerpts in order:\n1. before-secret.wav\n2. after-secret.wav\n\n"
                "Focus on the prominence of the lead and bass."
            ),
        },
        wav_part(np.full(1600, 0.2)),
        wav_part(np.full(1600, 0.05)),
    )
    values["max_tokens"] = 255
    request = server.parse_request(values, server.DEFAULT_MODEL)
    seen = []

    def observe(self, single):
        seen.append(single)
        assert len(single.audio) == 1
        assert self._generation_options(single)["use_cache"] is True
        assert self._processor_options() == {
            "text_kwargs": {"padding": True},
            "audio_kwargs": {"padding": "max_length"},
        }
        return server.ReviewResult(f"Independent answer {len(seen)}", 30, 10)

    monkeypatch.setattr(server.QwenBackend, "_generate", observe)
    backend = server.MusicFlamingoBackend()
    result = backend.generate(request)
    assert seen[0].audio[0] is request.audio[0]
    assert seen[1].audio[0] is request.audio[1]
    assert seen[0].messages == seen[1].messages
    rendered = json.dumps(seen[0].messages)
    assert "Only one audio excerpt" in rendered
    assert "Focus on the prominence of the lead and bass." in rendered
    assert "secret.wav" not in rendered
    assert "Independent answer" not in rendered
    assert [item.max_tokens for item in seen] == [128, 127]
    assert "Excerpt 1:\nIndependent answer 1" in result.text
    assert "Excerpt 2:\nIndependent answer 2" in result.text
    assert "no direct A/B judgment" in result.text
    assert result.prompt_tokens == 60
    assert result.completion_tokens == 20


def test_music_flamingo_single_preserves_request_and_qwen_cache_default(monkeypatch):
    request = server.parse_request(payload(), server.DEFAULT_MODEL)
    seen = []

    def observe(self, single):
        seen.append(single)
        return server.ReviewResult("Single unchanged")

    monkeypatch.setattr(server.QwenBackend, "_generate", observe)
    assert server.MusicFlamingoBackend().generate(request).text == "Single unchanged"
    assert seen == [request]
    assert "use_cache" not in server.QwenBackend()._generation_options(request)


def test_music_flamingo_rejects_wrong_native_model_before_loading_weights(monkeypatch):
    calls = []
    monkeypatch.setitem(
        sys.modules,
        "transformers",
        SimpleNamespace(
            AutoConfig=SimpleNamespace(
                from_pretrained=lambda *args, **kwargs: SimpleNamespace(model_type="qwen2_audio")
            )
        ),
    )
    monkeypatch.setattr(server.QwenBackend, "_load", lambda _: calls.append("weights"))
    with pytest.raises(RuntimeError, match="native musicflamingo"):
        server.MusicFlamingoBackend()._load()
    assert calls == []


def test_music_flamingo_pair_propagates_truncation_and_failure(monkeypatch):
    request = server.parse_request(payload(wav_part(), wav_part()), server.DEFAULT_MODEL)
    monkeypatch.setattr(
        server.QwenBackend,
        "_generate",
        lambda *_: server.ReviewResult("Incomplete", 25, 256, "length"),
    )
    assert server.MusicFlamingoBackend().generate(request).finish_reason == "length"

    def fail(*_):
        raise RuntimeError("Second model request failed")

    monkeypatch.setattr(server.QwenBackend, "_generate", fail)
    with pytest.raises(RuntimeError, match="model request failed"):
        server.MusicFlamingoBackend().generate(request)
