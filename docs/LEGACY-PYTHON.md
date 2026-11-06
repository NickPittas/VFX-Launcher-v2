# Legacy Python application

The Python/PySide6 application remains in `ui_slick/` and `core/` during the Rust migration. It has a separate configuration and catalog workflow; do not treat its installer or settings as instructions for the Rust desktop.

For the historical Windows build recipe, see [Python build instructions](../BUILD_INSTRUCTIONS.md#legacy-python-build). Run from the repository root with `python ui_slick/main_slick.py` after installing `requirements.txt` in an isolated Python environment. Configure the legacy database and Nuke/After Effects paths in that application.

For current packaged downloads and page guides, start at the [README](../README.md).
