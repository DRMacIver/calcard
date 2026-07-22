"""vobject-rs vendored copy: upstream's funding.json checks are repository
metadata tests for the collective/icalendar repository itself (they verify
that repo's FLOSS/fund manifest). They do not test library behavior, and
carrying icalendar's funding manifest at this repository's root would
misrepresent it, so they are skipped here. See VENDORED-NOTICE.txt.
"""

import pytest

pytest.skip(
    "upstream repository-metadata test; not applicable to the vendored copy",
    allow_module_level=True,
)
