#!/usr/bin/env python3
import runpy
from pathlib import Path
runpy.run_path(str(Path(__file__).resolve().parents[1] / "macos/scripts/check-statusline.py"), run_name="__main__")
