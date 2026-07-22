# The vendored upstream suite imports `vobject`; importing the compat
# package installs the alias mapping that name onto it.
import calcard.compat.pyvobject  # noqa: F401
