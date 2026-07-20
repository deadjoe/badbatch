#!/usr/bin/env python3
"""Create a temporary, probe-enabled LMAX SingleProducerSequencer source."""

from __future__ import annotations

import argparse
from pathlib import Path


ORIGINAL = """            long minSequence;
            while (wrapPoint > (minSequence = Util.getMinimumSequence(gatingSequences, nextValue)))
            {
                LockSupport.parkNanos(1L); // TODO: Use waitStrategy to spin?
            }

            this.cachedValue = minSequence;
"""

INSTRUMENTED = """            long minSequence;
            if (H2HDiagnostics.enabled())
            {
                long h2hBackpressureIterations = 0L;
                while (wrapPoint > (minSequence = Util.getMinimumSequence(gatingSequences, nextValue)))
                {
                    h2hBackpressureIterations++;
                    LockSupport.parkNanos(1L); // TODO: Use waitStrategy to spin?
                }
                H2HDiagnostics.recordProducerBackpressure(h2hBackpressureIterations);
            }
            else
            {
                while (wrapPoint > (minSequence = Util.getMinimumSequence(gatingSequences, nextValue)))
                {
                    LockSupport.parkNanos(1L); // TODO: Use waitStrategy to spin?
                }
            }

            this.cachedValue = minSequence;
"""


def instrument(source: str) -> str:
    """Patch exactly one known upstream loop, failing closed on source drift."""
    occurrences = source.count(ORIGINAL)
    if occurrences != 1:
        raise ValueError(
            "expected exactly one known SingleProducerSequencer wait loop, "
            f"found {occurrences}"
        )
    result = source.replace(ORIGINAL, INSTRUMENTED, 1)
    if result.count("H2HDiagnostics.recordProducerBackpressure") != 1:
        raise AssertionError("instrumentation marker count is not one")
    return result


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("input", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()

    source = args.input.read_text(encoding="utf-8")
    output = instrument(source)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(output, encoding="utf-8")


if __name__ == "__main__":
    main()
