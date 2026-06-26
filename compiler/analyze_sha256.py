import json
import os

content_types = set()
strategies = set()
missing_sha = []
has_sha = set()

for root, dirs, files in os.walk("registry"):
    dirs[:] = [d for d in dirs if d != "archived"]
    for f in files:
        if not f.endswith(".json"):
            continue
        path = os.path.join(root, f)
        try:
            with open(path) as fh:
                data = json.load(fh)
            if isinstance(data, dict):
                ct = data.get("content_type", "unknown")
                st = data.get("download_strategy", "none")
                content_types.add(ct)
                strategies.add(st)
                if data.get("sha256"):
                    has_sha.add(data["id"])
                else:
                    missing_sha.append((ct, st, data["id"], path))
        except Exception:
            pass

print("Content types:", sorted(content_types))
print("Strategies:", sorted(strategies))
print(f"Items with sha256: {len(has_sha)}")
print(f"Items missing sha256: {len(missing_sha)}")
print()
print("Missing sha256 by content type and strategy:")
for ct, st, item_id, path in sorted(missing_sha):
    print(f"  {ct:15s} {st:20s} {item_id}")
