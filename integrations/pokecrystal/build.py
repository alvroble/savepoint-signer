"""Build the Crystal seed ROM from the pinned upstream source."""
import argparse, hashlib, io, json, subprocess, tarfile, tempfile
from pathlib import Path
CONFIG=json.loads(Path(__file__).with_name("upstream.json").read_text())
PIN=CONFIG["commit"]
ROOT=Path(__file__).resolve().parents[2]
p=argparse.ArgumentParser()
p.add_argument("upstream",type=Path)
p.add_argument("rgbds",type=Path)
p.add_argument("--output",type=Path,default=ROOT/"target/crystal")
a=p.parse_args()
upstream=a.upstream.resolve();rgbds=a.rgbds.resolve();output=a.output.resolve()
version=subprocess.check_output([str(rgbds/"rgbasm"),"--version"],text=True).strip()
assert version=="rgbasm v"+CONFIG["rgbds"],version
patch=Path(__file__).with_name("crystal.patch")
output.mkdir(parents=True,exist_ok=True)
(ROOT/"target").mkdir(parents=True,exist_ok=True)
archive=subprocess.check_output(["git","-C",str(upstream),"archive",PIN])
with tempfile.TemporaryDirectory(prefix="crystal-build-",dir=ROOT/"target") as temp:
 source=Path(temp)
 with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
  tar.extractall(source,filter="data")
 subprocess.run(["patch","-p1","-i",str(patch)],cwd=source,check=True)
 (source/"engine/events/crystal_seed.asm").write_bytes(patch.with_name("native").joinpath("seed.asm").read_bytes())
 subprocess.run(["make","-j4","RGBDS="+str(rgbds)+"/","crystal"],cwd=source,check=True)
 for name in ("pokecrystal.gbc","pokecrystal.sym","pokecrystal.map"):
  (output/name).write_bytes((source/name).read_bytes())
manifest={"upstream":PIN,"rgbds":version,"integration":"crystal-seed","firmware_feature":"crystal-seed"}
manifest["sha256"]={name:hashlib.sha256((output/name).read_bytes()).hexdigest() for name in ("pokecrystal.gbc","pokecrystal.sym","pokecrystal.map")}
manifest["patch_sha256"]=hashlib.sha256(patch.read_bytes()).hexdigest()
manifest["seed_asm_sha256"]=hashlib.sha256(patch.with_name("native").joinpath("seed.asm").read_bytes()).hexdigest()
(output/"manifest.json").write_text(json.dumps(manifest,indent=2)+"\n")
print(output)
