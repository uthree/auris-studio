"""Neural network building blocks."""

from auris_singer.modules.alignment import maximum_path
from auris_singer.modules.discriminator import (
    Discriminator,
    MultiPeriodDiscriminator,
    MultiResolutionSTFTDiscriminator,
)
from auris_singer.modules.encoders import (
    PosteriorEncoder,
    PriorEncoder,
    TextEncoder,
)
from auris_singer.modules.flow import ResidualCouplingBlock
from auris_singer.modules.generator import NsfHifiGanGenerator
from auris_singer.modules.source import SourceSignalGenerator
from auris_singer.modules.transformer import TransformerEncoder

__all__ = [
    "maximum_path",
    "Discriminator",
    "MultiPeriodDiscriminator",
    "MultiResolutionSTFTDiscriminator",
    "PosteriorEncoder",
    "PriorEncoder",
    "TextEncoder",
    "ResidualCouplingBlock",
    "NsfHifiGanGenerator",
    "SourceSignalGenerator",
    "TransformerEncoder",
]
