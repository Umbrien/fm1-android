#!/usr/bin/env python3
"""Compile Clipper with the shared SLOOP-family host and isolated instance state."""
import sys
from build import main
if __name__ == '__main__':
    sys.argv.extend(['--kind', 'clipper'])
    main()
