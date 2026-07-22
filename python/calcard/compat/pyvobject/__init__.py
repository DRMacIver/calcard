"""py-vobject 1.0-compatible API, adapted from upstream py-vobject.

Importing this package installs the ``vobject`` import alias (see
``calcard.compat``), then exposes the upstream package surface:
``readComponents``, ``readOne``, ``newFromBehavior``, ``iCalendar``,
``vCard``, and the ``base``/``icalendar``/``vcard``/``hcalendar``/
``change_tz``/``ics_diff`` modules.
"""

from calcard.compat import install as _install
from calcard.compat import mirror as _mirror

_install()

# Importing icalendar and vcard registers their behaviors, which
# base.readComponents and newFromBehavior rely on.
from . import base
from . import icalendar
from . import vcard
from .base import VERSION, newFromBehavior, readComponents, readOne

__all__ = [
    "VERSION",
    "iCalendar",
    "newFromBehavior",
    "readComponents",
    "readOne",
    "vCard",
]


def iCalendar():
    """A new VCALENDAR 2.0 component."""
    return newFromBehavior("vcalendar", "2.0")


def vCard():
    """A new VCARD 3.0 component."""
    return newFromBehavior("vcard", "3.0")


# Mirror the modules imported above under the upstream `vobject` name so
# later alias imports are cache hits (see calcard.compat.mirror).
_mirror("vobject")
