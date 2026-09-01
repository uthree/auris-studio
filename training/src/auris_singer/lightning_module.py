"""Lightning module wiring the generator, the discriminators and the losses.

GAN training needs two optimizers stepped in a fixed order, so the module runs
with ``automatic_optimization = False``.
"""

from __future__ import annotations

import logging
from typing import Any

import lightning as L
import torch
import torch.nn.functional as F

from auris_singer.losses import (
    EnvelopeLoss,
    MultiParamMelLoss,
    discriminator_loss,
    feature_matching_loss,
    generator_adversarial_loss,
    kl_loss,
)
from auris_singer.metrics import energy_metrics, pitch_metrics
from auris_singer.model import AurisSinger
from auris_singer.modules.discriminator import Discriminator
from auris_singer.preprocess.f0 import FcpeExtractor
from auris_singer.utils.audio import frame_energy, mel_spectrogram
from auris_singer.utils.masks import sequence_mask, slice_segments

logger = logging.getLogger(__name__)

__all__ = ["AurisSingerModule"]


class AurisSingerModule(L.LightningModule):
    """Training wrapper.

    Args:
        model: keyword arguments for :class:`~auris_singer.model.AurisSinger`.
        discriminator: keyword arguments for
            :class:`~auris_singer.modules.discriminator.Discriminator`.
        audio: spectrogram settings, used for logging and the mel losses.
        loss: loss weights and per-loss settings.
        optimizer: learning rate, betas, weight decay and LR decay.
        metadata: dataset metadata (phoneme symbols, speaker map) stored in the
            checkpoint so inference needs nothing but the checkpoint file.
        validation: validation-time settings (source-control metrics, audio
            logging).
    """

    def __init__(
        self,
        model: dict[str, Any],
        discriminator: dict[str, Any] | None = None,
        audio: dict[str, Any] | None = None,
        loss: dict[str, Any] | None = None,
        optimizer: dict[str, Any] | None = None,
        metadata: dict[str, Any] | None = None,
        validation: dict[str, Any] | None = None,
    ):
        super().__init__()
        self.save_hyperparameters()
        self.automatic_optimization = False

        model = dict(model)
        audio = dict(audio or {})
        loss = dict(loss or {})
        optimizer = dict(optimizer or {})
        discriminator = dict(discriminator or {})

        self.sample_rate = int(audio.get("sample_rate", model.get("sample_rate", 48_000)))
        self.hop_length = int(audio.get("hop_length", model.get("hop_length", 480)))
        self.n_fft = int(audio.get("n_fft", 2048))
        self.win_length = int(audio.get("win_length", self.n_fft))
        self.n_mels = int(audio.get("n_mels", 128))
        self.f_min = float(audio.get("f_min", 0.0))
        self.f_max = audio.get("f_max", None)

        self.model = AurisSinger(**model)
        discriminator.setdefault("n_speakers", model.get("n_speakers", 1))
        self.discriminator = Discriminator(**discriminator)

        self.envelope_loss = EnvelopeLoss(
            kernel_sizes=tuple(loss.get("envelope_kernel_sizes", (128, 256, 512, 1024)))
        )
        mel_params = loss.get(
            "mel_params",
            (
                (512, 120, 512, 40),
                (1024, 240, 1024, 80),
                (2048, 480, 2048, 128),
                (4096, 960, 4096, 160),
            ),
        )
        self.mel_loss = MultiParamMelLoss(
            sample_rate=self.sample_rate,
            params=tuple(tuple(p) for p in mel_params),
            f_min=self.f_min,
            f_max=self.f_max,
        )

        self.weights = {
            "mel": float(loss.get("mel", 45.0)),
            "kl": float(loss.get("kl", 1.0)),
            # The auxiliary term only has to keep the alignment statistic honest.
            # At full weight it doubles the KL pressure relative to VITS, which
            # pushes the latent toward collapse.
            "kl_aux": float(loss.get("kl_aux", 0.2)),
            "feature_matching": float(loss.get("feature_matching", 1.0)),
            "envelope": float(loss.get("envelope", 10.0)),
            "adversarial": float(loss.get("adversarial", 1.0)),
        }
        self.kl_free_bits = float(loss.get("kl_free_bits", 0.02))
        self.kl_warmup_steps = int(loss.get("kl_warmup_steps", 10_000))

        self.learning_rate = float(optimizer.get("learning_rate", 2e-4))
        self.betas = tuple(optimizer.get("betas", (0.8, 0.99)))
        self.eps = float(optimizer.get("eps", 1e-9))
        self.weight_decay = float(optimizer.get("weight_decay", 0.0))
        self.lr_decay = float(optimizer.get("lr_decay", 0.999875))
        self.grad_clip = float(optimizer.get("grad_clip", 0.0))

        validation = dict(validation or {})
        self.pitch_metrics_enabled = bool(validation.get("pitch_metrics", True))
        self.metric_f0_min = float(validation.get("f0_min", 40.0))
        self.metric_f0_max = float(validation.get("f0_max", 1600.0))
        self.metric_tolerance_cents = float(validation.get("tolerance_cents", 50.0))
        self.log_audio_batches = int(validation.get("log_audio_batches", 4))
        self._pitch_extractor: FcpeExtractor | None = None
        self._pitch_extractor_failed = False
        self._logged_reference_audio: set[int] = set()

    # ------------------------------------------------------------------
    def configure_optimizers(self):
        opt_g = torch.optim.AdamW(
            self.model.parameters(),
            lr=self.learning_rate,
            betas=self.betas,
            eps=self.eps,
            weight_decay=self.weight_decay,
        )
        opt_d = torch.optim.AdamW(
            self.discriminator.parameters(),
            lr=self.learning_rate,
            betas=self.betas,
            eps=self.eps,
            weight_decay=self.weight_decay,
        )
        sch_g = torch.optim.lr_scheduler.ExponentialLR(opt_g, gamma=self.lr_decay)
        sch_d = torch.optim.lr_scheduler.ExponentialLR(opt_d, gamma=self.lr_decay)
        return [opt_g, opt_d], [sch_g, sch_d]

    # ------------------------------------------------------------------
    def _log_mel(self, wav: torch.Tensor) -> torch.Tensor:
        if wav.dim() == 3:
            wav = wav.squeeze(1)
        return mel_spectrogram(
            wav.float(),
            sample_rate=self.sample_rate,
            n_fft=self.n_fft,
            hop_length=self.hop_length,
            win_length=self.win_length,
            n_mels=self.n_mels,
            f_min=self.f_min,
            f_max=self.f_max,
        )

    def kl_scale(self) -> float:
        """Linear KL warm-up factor for the current step.

        The reconstruction path needs a head start: matching the prior is cheap
        and making the latent informative is expensive, so a KL applied at full
        weight from step 0 wins that race and the latent never recovers.
        """
        if self.kl_warmup_steps <= 0:
            return 1.0
        return min(1.0, self.global_step / self.kl_warmup_steps)

    def _clip(self, module: torch.nn.Module) -> None:
        if self.grad_clip > 0:
            torch.nn.utils.clip_grad_norm_(module.parameters(), self.grad_clip)

    # ------------------------------------------------------------------
    def training_step(self, batch: dict[str, torch.Tensor], batch_idx: int) -> None:
        opt_g, opt_d = self.optimizers()
        speaker_ids = batch["speaker_ids"]

        out = self.model(
            phonemes=batch["phonemes"],
            phoneme_lengths=batch["phoneme_lengths"],
            spec=batch["spec"],
            spec_lengths=batch["spec_lengths"],
            f0=batch["f0"],
            energy=batch["energy"],
            voiced=batch["voiced"],
            speaker_ids=speaker_ids,
            # Labelled where the corpus had labels and the data module kept them; the
            # alignment search otherwise.
            durations=batch.get("durations"),
        )
        wav_hat = out["wav_hat"]
        segment_samples = wav_hat.size(-1)
        wav_real = slice_segments(
            batch["wav"], out["slice_ids"] * self.hop_length, segment_samples
        )

        # --- discriminator ------------------------------------------------
        real_out, _ = self.discriminator(wav_real, speaker_ids)
        fake_out, _ = self.discriminator(wav_hat.detach(), speaker_ids)
        loss_disc, _, _ = discriminator_loss(real_out, fake_out)

        opt_d.zero_grad(set_to_none=True)
        self.manual_backward(loss_disc)
        self._clip(self.discriminator)
        opt_d.step()

        # --- generator ----------------------------------------------------
        real_out, real_fmap = self.discriminator(wav_real, speaker_ids)
        fake_out, fake_fmap = self.discriminator(wav_hat, speaker_ids)

        loss_adv, _ = generator_adversarial_loss(fake_out)
        loss_fm = feature_matching_loss(real_fmap, fake_fmap)
        loss_mel = self.mel_loss(wav_real, wav_hat)
        loss_env = self.envelope_loss(wav_real, wav_hat)
        loss_kl = kl_loss(
            out["z_p"],
            out["logs_q"],
            out["m_p"],
            out["logs_p"],
            out["y_mask"],
            free_bits=self.kl_free_bits,
        )
        # The auxiliary term is the objective monotonic alignment search
        # maximizes; keeping it in the loss stops the alignment prior from
        # drifting away from the refined prior. No free bits here — this term
        # is about the alignment statistic, not about the latent's capacity.
        loss_kl_aux = kl_loss(
            out["z_p"],
            out["logs_q"],
            out["m_p0_frame"],
            out["logs_p0_frame"],
            out["y_mask"],
        )

        kl_scale = self.kl_scale()

        loss_gen = (
            self.weights["adversarial"] * loss_adv
            + self.weights["feature_matching"] * loss_fm
            + self.weights["mel"] * loss_mel
            + self.weights["envelope"] * loss_env
            + kl_scale * self.weights["kl"] * loss_kl
            + kl_scale * self.weights["kl_aux"] * loss_kl_aux
        )

        opt_g.zero_grad(set_to_none=True)
        self.manual_backward(loss_gen)
        self._clip(self.model)
        opt_g.step()

        self.log_dict(
            {
                "train/loss_disc": loss_disc,
                "train/loss_gen": loss_gen,
                "train/adv": loss_adv,
                "train/feature_matching": loss_fm,
                "train/mel": loss_mel,
                "train/envelope": loss_env,
                "train/kl": loss_kl,
                "train/kl_aux": loss_kl_aux,
                "train/kl_scale": torch.tensor(kl_scale, device=loss_kl.device),
                # Collapse watch: if sigma drifts up while the mean flattens,
                # the latent is turning into noise and the decoder is falling
                # back on the excitation signal alone.
                "train/posterior_sigma": out["logs_q"].detach().float().exp().mean(),
                "train/posterior_mean_rms": out["m_q"].detach().float().pow(2).mean().sqrt(),
            },
            prog_bar=False,
            on_step=True,
            on_epoch=False,
        )
        self.log("train/loss", loss_gen, prog_bar=True, on_step=True, on_epoch=False)

    def on_train_epoch_end(self) -> None:
        for scheduler in self.lr_schedulers():
            scheduler.step()

    # ------------------------------------------------------------------
    @torch.no_grad()
    def validation_step(self, batch: dict[str, torch.Tensor], batch_idx: int) -> None:
        speaker_ids = batch["speaker_ids"]
        out = self.model(
            phonemes=batch["phonemes"],
            phoneme_lengths=batch["phoneme_lengths"],
            spec=batch["spec"],
            spec_lengths=batch["spec_lengths"],
            f0=batch["f0"],
            energy=batch["energy"],
            voiced=batch["voiced"],
            speaker_ids=speaker_ids,
            # Labelled where the corpus had labels and the data module kept them; the
            # alignment search otherwise.
            durations=batch.get("durations"),
        )
        durations = out["durations"].round().long()

        wav_hat = self.model.infer(
            phonemes=batch["phonemes"],
            phoneme_lengths=batch["phoneme_lengths"],
            durations=durations,
            f0=batch["f0"],
            energy=batch["energy"],
            voiced=batch["voiced"],
            speaker_ids=speaker_ids,
        )
        wav_real = batch["wav"]
        length = min(wav_hat.size(-1), wav_real.size(-1))
        mel_hat = self._log_mel(wav_hat[..., :length])
        mel_real = self._log_mel(wav_real[..., :length])
        loss_mel = F.l1_loss(mel_hat, mel_real)

        self.log("val/mel", loss_mel, prog_bar=True, on_epoch=True, sync_dist=True)
        self.log(
            "val/latent_usage",
            self._latent_usage(batch, out),
            on_epoch=True,
            sync_dist=True,
        )
        self._log_source_control_metrics(batch, wav_hat[..., :length])
        if batch_idx < self.log_audio_batches:
            self._log_audio(batch_idx, wav_hat[0, :, :length], wav_real[0, :, :length])

    @torch.no_grad()
    def _latent_usage(
        self, batch: dict[str, torch.Tensor], out: dict[str, torch.Tensor]
    ) -> torch.Tensor:
        """How much worse the decoder gets when ``z`` is shuffled along time.

        Phonetic content can only reach the decoder through ``z``; pitch and
        loudness arrive separately through the excitation. So if permuting
        ``z`` in time costs nothing, the latent is carrying no time-varying
        information and the decoder is running on the excitation alone — the
        posterior has collapsed, and the output will track pitch perfectly
        while saying nothing.

        The excitation from the intact run is reused rather than regenerated:
        it is stochastic, so a fresh one would put a noise floor under the
        metric and make a collapsed model score above 0.

        Returns:
            ``mel_L1(shuffled z) - mel_L1(z)``. Well above 0 means the latent
            is doing work; near 0 means it has collapsed.
        """
        z_slice = out["z_slice"]
        segment = z_slice.size(-1)
        if segment < 2:
            return torch.zeros((), device=z_slice.device)

        permutation = torch.randperm(segment, device=z_slice.device)
        shuffled, _ = self.model.generator(
            z_slice[:, :, permutation],
            out["f0_slice"],
            out["energy_slice"],
            out["voiced_slice"],
            g=out["g"],
            source=out["source"],
        )
        wav_real = slice_segments(
            batch["wav"], out["slice_ids"] * self.hop_length, out["wav_hat"].size(-1)
        )
        mel_real = self._log_mel(wav_real)
        intact = F.l1_loss(self._log_mel(out["wav_hat"]), mel_real)
        permuted = F.l1_loss(self._log_mel(shuffled), mel_real)
        return permuted - intact

    # ------------------------------------------------------------------
    @property
    def pitch_extractor(self) -> FcpeExtractor | None:
        """Lazily built FCPE extractor used to re-analyse generated audio."""
        if self._pitch_extractor_failed or not self.pitch_metrics_enabled:
            return None
        if self._pitch_extractor is None or str(self._pitch_extractor.device) != str(
            self.device
        ):
            try:
                extractor = FcpeExtractor(
                    device=str(self.device),
                    f0_min=self.metric_f0_min,
                    f0_max=self.metric_f0_max,
                )
                extractor.model  # force construction here, not mid-metric
                self._pitch_extractor = extractor
            except Exception as exc:  # pragma: no cover - depends on environment
                logger.warning("pitch metrics disabled, FCPE unavailable: %s", exc)
                self._pitch_extractor_failed = True
                return None
        return self._pitch_extractor

    @torch.no_grad()
    def _log_source_control_metrics(
        self, batch: dict[str, torch.Tensor], wav_hat: torch.Tensor
    ) -> None:
        """Measure how closely the output follows the requested f0 and energy.

        Pitch and loudness reach the decoder only through the excitation
        signal, so re-analysing the generated waveform and comparing it with
        the input curves directly measures whether that control works.
        """
        n_frames = wav_hat.size(-1) // self.hop_length
        if n_frames < 2:
            return
        wav = wav_hat.squeeze(1).float()

        valid = sequence_mask(
            batch["spec_lengths"].clamp(max=n_frames), n_frames
        ).float()

        pred_energy = frame_energy(wav, self.n_fft, self.hop_length, self.win_length)
        metrics = energy_metrics(batch["energy"][:, :n_frames], pred_energy, valid)

        extractor = self.pitch_extractor
        if extractor is not None:
            try:
                pred_f0, pred_voiced = extractor(wav, self.sample_rate, n_frames)
            except Exception as exc:  # pragma: no cover - depends on environment
                logger.warning("pitch metrics disabled, FCPE failed: %s", exc)
                self._pitch_extractor_failed = True
            else:
                metrics.update(
                    pitch_metrics(
                        target_f0=batch["f0"][:, :n_frames],
                        target_voiced=batch["voiced"][:, :n_frames],
                        pred_f0=pred_f0.to(wav.device),
                        pred_voiced=pred_voiced.to(wav.device),
                        valid=valid,
                        tolerance_cents=self.metric_tolerance_cents,
                    )
                )

        # A metric with no frames to average over is NaN; logging it would
        # poison the epoch mean, so those are dropped instead.
        finite = {
            f"val/{name}": value
            for name, value in metrics.items()
            if torch.isfinite(value)
        }
        if finite:
            self.log_dict(finite, on_epoch=True, sync_dist=True)

    def _log_audio(
        self, index: int, wav_hat: torch.Tensor, wav_real: torch.Tensor
    ) -> None:
        logger = self.logger
        experiment = getattr(logger, "experiment", None)
        if experiment is None or not hasattr(experiment, "add_audio"):
            return
        experiment.add_audio(
            f"val/{index}/generated",
            wav_hat.float().cpu(),
            self.global_step,
            self.sample_rate,
        )
        # The reference never changes, so log it once per utterance. Keying on
        # the first validation pass rather than on epoch 0 matters: with a
        # step-based `val_check_interval`, epoch 0 is usually long gone by the
        # time validation first runs.
        if index not in self._logged_reference_audio:
            self._logged_reference_audio.add(index)
            experiment.add_audio(
                f"val/{index}/reference",
                wav_real.float().cpu(),
                self.global_step,
                self.sample_rate,
            )
