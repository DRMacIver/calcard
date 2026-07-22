"""Compatibility layers for py-vobject and icalendar.

``calcard.compat.pyvobject`` is a py-vobject 1.0-compatible package and
``calcard.compat.icalendar`` an icalendar 7.2.2-compatible one (both
adapted from their upstreams; see DESIGN.md for the policy and the
LICENSES/ directory for attribution).

The compat code — and any existing application code written against the
upstream libraries — refers to the packages by their upstream names
(``vobject``, ``icalendar``). Importing either compat package installs an
import alias mapping those names onto the compat packages, so upstream
code and test suites run unchanged. The aliasing is deliberately opt-in:
nothing is touched until ``calcard.compat`` (or one of its subpackages)
is imported, so installing calcard never shadows a separately installed
real py-vobject or icalendar distribution unless you ask for it.
"""

from __future__ import annotations

import importlib
import importlib.abc
import importlib.util
import sys

_ALIASES = {
    "vobject": "calcard.compat.pyvobject",
    "icalendar": "calcard.compat.icalendar",
}


class _AliasLoader(importlib.abc.Loader):
    """Loads an aliased name by importing the canonical module and reusing
    the very same module object, so ``vobject.base`` and
    ``calcard.compat.pyvobject.base`` are one instance, not two copies."""

    def __init__(self, real_name: str):
        self.real_name = real_name
        self._identity = None

    def create_module(self, spec):
        module = importlib.import_module(self.real_name)
        # The machinery will stamp the alias spec onto the shared object;
        # remember its canonical identity so exec_module can restore it
        # (otherwise every relative import inside warns about
        # __package__ != __spec__.parent and resolves inconsistently).
        self._identity = (
            module.__name__,
            module.__dict__.get("__spec__"),
            module.__dict__.get("__package__"),
        )
        return module

    def exec_module(self, module):
        # The canonical import already executed the module; restore its
        # canonical identity.
        name, spec, package = self._identity
        module.__name__ = name
        if spec is not None:
            module.__spec__ = spec
        if package is not None:
            module.__package__ = package


class _AliasFinder(importlib.abc.MetaPathFinder):
    def find_spec(self, fullname, path=None, target=None):
        for alias, real in _ALIASES.items():
            if fullname == alias or fullname.startswith(alias + "."):
                real_name = real + fullname[len(alias):]
                try:
                    real_spec = importlib.util.find_spec(real_name)
                except ModuleNotFoundError:
                    return None
                if real_spec is None:
                    return None
                return importlib.util.spec_from_loader(
                    fullname,
                    _AliasLoader(real_name),
                    origin=real_spec.origin,
                    is_package=real_spec.submodule_search_locations is not None,
                )
        return None


_finder = _AliasFinder()


def install() -> None:
    """Install the ``vobject``/``icalendar`` import aliases (idempotent).

    Called automatically when either compat package is imported. After
    installation, ``import vobject`` and ``import icalendar`` (and any
    submodule) resolve to the calcard compat packages, shadowing any
    other installed distribution of those names for this process.
    """
    if _finder not in sys.meta_path:
        # Ahead of the path-based finders so alias submodule imports are
        # intercepted before the parent package's __path__ is searched
        # (which would create duplicate module instances).
        sys.meta_path.insert(0, _finder)


def mirror(alias: str) -> None:
    """Pre-seed ``sys.modules`` alias entries for every already-imported
    canonical submodule of an aliased package.

    Called by the compat packages at the end of their own ``__init__``:
    with the aliases cached, a later ``import icalendar.timezone.tzp`` is a
    plain cache hit instead of a fresh load — which matters because a fresh
    load re-runs the parent-attribute assignment and would clobber package
    attributes that deliberately shadow a submodule name (upstream
    icalendar's ``timezone.tzp`` instance, for example)."""
    real = _ALIASES[alias]
    prefix = real + "."
    for name, module in list(sys.modules.items()):
        if name == real or name.startswith(prefix):
            sys.modules.setdefault(alias + name[len(real):], module)


def uninstall() -> None:
    """Remove the import aliases installed by :func:`install`.

    Modules already imported through the aliases stay in ``sys.modules``;
    this only stops future imports from resolving through calcard.
    """
    if _finder in sys.meta_path:
        sys.meta_path.remove(_finder)
