"""Package an already-built firmware with the publishable source tree."""
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[1]
FIRMWARE = ROOT / "target/thumbv8m.main-none-eabihf/release/rp2350-gameboy-cartridge"
OUT = ROOT / "target/release"

def git(*args):
    return subprocess.check_output(["git", "-C", str(ROOT), *args])


def main():
    firmware = FIRMWARE.read_bytes()
    if not firmware.startswith(b"\x7fELF"):
        raise SystemExit("Build the firmware ELF before packaging")
    files = sorted(set(git("ls-files", "-z", "--cached", "--others", "--exclude-standard").decode().split("\0")))
    payload = {}
    for name in files:
        if not name:
            continue
        path = ROOT / name
        # Tracked deletions stay in the index until the user stages them.
        if not path.is_file():
            continue
        if path.is_symlink() or ".." in Path(name).parts:
            raise SystemExit(f"Unexpected source path: {name}")
        if path.suffix.lower() in {".gb", ".gbc", ".sav", ".ram", ".uf2", ".elf", ".psbt"}:
            raise SystemExit(f"Do not publish generated/private artifact: {name}")
        payload[name] = path.read_bytes()
    payload["firmware/rp2350-gameboy-cartridge.elf"] = firmware
    manifest = {
        "commit": git("rev-parse", "HEAD").decode().strip(),
        "dirty": bool(git("status", "--porcelain").strip()),
        "project": "Savepoint Signer",
        "integration": "crystal-seed",
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "sha256": {name: hashlib.sha256(data).hexdigest() for name, data in payload.items()},
    }
    payload["release-manifest.json"] = (json.dumps(manifest, indent=2) + "\n").encode()
    OUT.mkdir(parents=True, exist_ok=True)
    destination = OUT / "savepoint-signer-source-firmware.tar.gz"
    with tarfile.open(destination, "w:gz") as archive:
        for name, data in payload.items():
            entry = tarfile.TarInfo("savepoint-signer/" + name)
            entry.size = len(data)
            entry.mode = 0o755 if name.endswith(".sh") else 0o644
            archive.addfile(entry, io.BytesIO(data))
    (OUT / "SHA256SUMS").write_text(hashlib.sha256(destination.read_bytes()).hexdigest() + "  " + destination.name + "\n")
    print(destination)
    print(f"{len(payload)} files; source dirty={manifest['dirty']}")

if __name__ == "__main__":
    main()
