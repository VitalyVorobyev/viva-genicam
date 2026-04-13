"""NodeInfo dataclass + NodeKind enumeration."""

from __future__ import annotations

from dataclasses import asdict, dataclass
from enum import Enum
from typing import Optional


class NodeKind(str, Enum):
    """GenICam node kinds matching ``Node::kind_name()`` on the Rust side."""

    INTEGER = "Integer"
    FLOAT = "Float"
    ENUMERATION = "Enumeration"
    BOOLEAN = "Boolean"
    COMMAND = "Command"
    CATEGORY = "Category"
    SWISS_KNIFE = "SwissKnife"
    CONVERTER = "Converter"
    INT_CONVERTER = "IntConverter"
    STRING_REG = "StringReg"

    @classmethod
    def _coerce(cls, value: str) -> "NodeKind | str":
        try:
            return cls(value)
        except ValueError:
            return value


Access = Optional[str]  # "RO" | "RW" | "WO" | None (for categories)
Visibility = str  # "Beginner" | "Expert" | "Guru" | "Invisible"


@dataclass(frozen=True)
class NodeInfo:
    """Metadata about a GenICam node (feature)."""

    name: str
    kind: str
    access: Access
    visibility: Visibility
    display_name: Optional[str]
    description: Optional[str]
    tooltip: Optional[str]

    @classmethod
    def from_dict(cls, d: dict) -> "NodeInfo":
        return cls(
            name=d["name"],
            kind=d["kind"],
            access=d.get("access"),
            visibility=d.get("visibility", "Beginner"),
            display_name=d.get("display_name"),
            description=d.get("description"),
            tooltip=d.get("tooltip"),
        )

    def to_dict(self) -> dict:
        return asdict(self)

    @property
    def writable(self) -> bool:
        return self.access in {"RW", "WO"}

    @property
    def readable(self) -> bool:
        return self.access in {"RO", "RW"}


__all__ = ["NodeKind", "NodeInfo"]
