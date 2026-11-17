"""Check UI capture geometry and appearance while retaining exact pixel diagnostics."""
import argparse
import json
import math
from pathlib import Path


# One percent of an 8-bit channel's range. This is an automated image sanity
# check alongside the input/geometry contracts and visual review, not a claim
# that equal mean errors imply equal interfaces.
MAX_MEAN_CHANNEL_ERROR = 2.5


def comparison_errors(records, expected_count, *, require_content=False, strict_pixels=False):
    errors = []
    if not isinstance(records, dict) or "status" in records:
        return ["Capture comparison is missing or inconclusive"]
    if len(records) != expected_count:
        errors.append(f"Expected {expected_count} scenes, received {len(records)}")
    for name, record in records.items():
        if not isinstance(record, dict) or "error" in record:
            errors.append(f"{name}: capture or comparison failed")
            continue
        dimensions = [record.get(key) for key in ("originalWidth", "originalHeight", "width", "height")]
        if any(type(value) is not int or value <= 0 for value in dimensions):
            errors.append(f"{name}: invalid dimensions")
            continue
        if dimensions[:2] != dimensions[2:]:
            errors.append(f"{name}: window dimensions differ")
        for field in ("originalHasContent", "candidateHasContent"):
            if (require_content or field in record) and record.get(field) is not True:
                errors.append(f"{name}: {field} is not confirmed")
        mean = record.get("meanChannelError")
        if (type(mean) not in (int, float) or not math.isfinite(mean)
                or not 0 <= mean <= MAX_MEAN_CHANNEL_ERROR):
            errors.append(f"{name}: mean channel error exceeds {MAX_MEAN_CHANNEL_ERROR}/255 or is invalid")
        changed = record.get("changedPixels")
        area = max(dimensions[0] * dimensions[1], dimensions[2] * dimensions[3])
        if type(changed) is not int or not 0 <= changed <= area:
            errors.append(f"{name}: invalid changed-pixel count")
        elif strict_pixels and changed != 0:
            errors.append(f"{name}: pixels are not identical")
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    parser.add_argument("--expected-count", type=int, required=True)
    parser.add_argument("--require-content", action="store_true")
    parser.add_argument("--strict-pixels", action="store_true")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.expected_count <= 0:
        parser.error("--expected-count must be positive")
    records = json.loads(args.report.read_text("utf-8-sig"))
    errors = comparison_errors(records, args.expected_count, require_content=args.require_content,
                               strict_pixels=args.strict_pixels)
    result = {"passed": not errors, "expectedScenes": args.expected_count,
              "strictPixels": args.strict_pixels, "maxMeanChannelError": MAX_MEAN_CHANNEL_ERROR,
              "errors": errors}
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf8")
    print(json.dumps(result, ensure_ascii=False))
    return int(bool(errors))


if __name__ == "__main__":
    raise SystemExit(main())
