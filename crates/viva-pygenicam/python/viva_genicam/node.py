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
    REGISTER = "Register"

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
    #: The access mode the device permits *right now*, as opposed to ``access``,
    #: which is what the XML declares. They differ whenever ``pIsLocked`` is
    #: engaged: a node with no declared ``<AccessMode>`` defaults to ``"RW"``
    #: yet may still refuse writes.
    #:
    #: Only populated by :meth:`Camera.node_info`. :meth:`Camera.all_node_info`
    #: leaves it ``None``, because resolving it needs a register read per node.
    effective_access: Optional[Access] = None

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
            effective_access=d.get("effective_access"),
        )

    def to_dict(self) -> dict:
        return asdict(self)

    @property
    def writable(self) -> bool:
        return self.access in {"RW", "WO"}

    @property
    def readable(self) -> bool:
        return self.access in {"RO", "RW"}


@dataclass(frozen=True)
class SkippedNode:
    """A declaration the camera's XML has that the library could not expose.

    A feature missing from :meth:`Camera.nodes` may be one the camera does not
    have, or one listed here; the ``error`` says why. Worth reporting.
    """

    #: The XML element, such as ``"Register"``.
    tag: str
    #: The node's ``Name``, if the declaration had one.
    name: Optional[str]
    #: Why it could not be built.
    error: str

    @classmethod
    def from_dict(cls, d: dict) -> "SkippedNode":
        return cls(tag=d["tag"], name=d.get("name"), error=d["error"])


__all__ = ["NodeKind", "NodeInfo"]
