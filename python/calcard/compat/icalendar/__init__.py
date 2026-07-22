# calcard vendored copy: install the `icalendar`/`vobject` import aliases
# before anything else — this package's own modules (and its test suite)
# import each other by the upstream absolute name.
from calcard.compat import install as _install_compat_aliases

_install_compat_aliases()
del _install_compat_aliases

from icalendar.alarms import (
    Alarms,
    AlarmTime,
)
from icalendar.cal import (
    Alarm,
    Availability,
    Available,
    Calendar,
    Component,
    ComponentFactory,
    Event,
    FreeBusy,
    Journal,
    LazyCalendar,
    Timezone,
    TimezoneDaylight,
    TimezoneStandard,
    Todo,
)
from icalendar.enums import (
    BUSYTYPE,
    CLASS,
    CUTYPE,
    FBTYPE,
    PARTSTAT,
    RANGE,
    RELATED,
    RELTYPE,
    ROLE,
    STATUS,
    TRANSP,
    VALUE,
)
from icalendar.error import (
    BrokenCalendarProperty,
    ComponentEndMissing,
    ComponentStartMissing,
    FeatureWillBeRemovedInFutureVersion,
    ICalParsingError,
    IncompleteAlarmInformation,
    IncompleteComponent,
    InvalidCalendar,
    JCalParsingError,
    LocalTimezoneMissing,
)

# Parameters and helper methods for splitting and joining string with escaped
# chars.
from icalendar.parser import (
    Parameters,
    q_join,
    q_split,
)

# Property Data Value Types
from icalendar.prop import (
    VPROPERTY,
    AdrFields,
    TypesFactory,
    vAdr,
    vBinary,
    vBoolean,
    vBroken,
    vCalAddress,
    vCategory,
    vDate,
    vDatetime,
    vDDDLists,
    vDDDTypes,
    vDuration,
    vFloat,
    vFrequency,
    vGeo,
    vInt,
    vMonth,
    vN,
    vOrg,
    vPeriod,
    vRecur,
    vSkip,
    vText,
    vTime,
    vUid,
    vUnknown,
    vUri,
    vUTCOffset,
    vWeekday,
    vXmlReference,
)
from icalendar.prop.conference import Conference
from icalendar.prop.image import Image

# Switching the timezone provider
from icalendar.prop.n import NFields
from icalendar.timezone import is_utc, use_pytz, use_zoneinfo

from .version import __version__, __version_tuple__, version, version_tuple

__all__ = [
    "BUSYTYPE",
    "CLASS",
    "CUTYPE",
    "FBTYPE",
    "PARTSTAT",
    "RANGE",
    "RELATED",
    "RELTYPE",
    "ROLE",
    "STATUS",
    "TRANSP",
    "VALUE",
    "VPROPERTY",
    "AdrFields",
    "Alarm",
    "AlarmTime",
    "Alarms",
    "Availability",
    "Available",
    "BrokenCalendarProperty",
    "Calendar",
    "Component",
    "ComponentEndMissing",
    "ComponentFactory",
    "ComponentStartMissing",
    "Conference",
    "Event",
    "FeatureWillBeRemovedInFutureVersion",
    "FreeBusy",
    "ICalParsingError",
    "Image",
    "IncompleteAlarmInformation",
    "IncompleteComponent",
    "InvalidCalendar",
    "JCalParsingError",
    "Journal",
    "LazyCalendar",
    "LocalTimezoneMissing",
    "NFields",
    "Parameters",
    "Timezone",
    "TimezoneDaylight",
    "TimezoneStandard",
    "Todo",
    "TypesFactory",
    "__version__",
    "__version_tuple__",
    "is_utc",
    "q_join",
    "q_split",
    "use_pytz",
    "use_zoneinfo",
    "vAdr",
    "vBinary",
    "vBoolean",
    "vBroken",
    "vCalAddress",
    "vCategory",
    "vDDDLists",
    "vDDDTypes",
    "vDate",
    "vDatetime",
    "vDuration",
    "vFloat",
    "vFrequency",
    "vGeo",
    "vInt",
    "vMonth",
    "vN",
    "vOrg",
    "vPeriod",
    "vRecur",
    "vSkip",
    "vText",
    "vTime",
    "vUTCOffset",
    "vUid",
    "vUnknown",
    "vUri",
    "vWeekday",
    "vXmlReference",
    "version",
    "version_tuple",
]


# calcard vendored copy: mirror every submodule loaded above into
# sys.modules under the upstream `icalendar` name, so later alias imports
# are cache hits (see calcard.compat.mirror).
from calcard.compat import mirror as _mirror_compat_aliases

_mirror_compat_aliases("icalendar")
del _mirror_compat_aliases
