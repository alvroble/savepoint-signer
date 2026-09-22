"""Record the New Bark sign-to-signer path from the tested Crystal ROM.

The emulator-only start fixture skips Crystal's introductory player setup.
No seed is entered and no signing command is sent.
"""

import argparse
import subprocess
import tempfile
from pathlib import Path

from pyboy import PyBoy


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("build", type=Path, help="directory with pokecrystal.gbc/.sym")
    parser.add_argument("oracle", type=Path, help="crystal_ui_oracle test executable")
    parser.add_argument("output", type=Path, help="MP4 to write")
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)

    symbols = {}
    for line in (args.build / "pokecrystal.sym").read_text().splitlines():
        fields = line.split()
        if len(fields) == 2 and ":" in fields[0]:
            symbols[fields[1]] = tuple(int(part, 16) for part in fields[0].split(":"))

    def addr(name):
        return symbols[name][1]

    with tempfile.TemporaryDirectory(prefix="savepoint-demo-") as temporary:
        battery = Path(temporary) / "demo.ram"
        battery.write_bytes(bytes(0x8000))
        with battery.open("rb+") as ram_file:
            emulator = PyBoy(
                str(args.build / "pokecrystal.gbc"),
                window="null",
                cgb=True,
                sound_emulated=False,
                log_level="ERROR",
                ram_file=ram_file,
            )
            emulator.set_emulation_speed(0)
            oracle = None
            encoder = None
            try:
                # Match test_seed.py's boot fixture, starting a few steps away.
                for routine in ("PlayerProfileSetup", "OakSpeech", "ShrinkPlayer"):
                    bank, pc = symbols[routine]
                    emulator.memory[bank, pc] = 0xC9
                bank, pc = symbols["SpawnPoints"]
                emulator.memory[bank, pc : pc + 4] = [24, 4, 9, 10]
                emulator.tick(1000)

                def tap_unrecorded(key):
                    emulator.button_press(key)
                    emulator.tick(2)
                    emulator.button_release(key)
                    emulator.tick(100)

                for _ in range(15):
                    tap_unrecorded("a")
                    emulator.tick(120)
                    if (emulator.memory[addr("wMapGroup")], emulator.memory[addr("wMapNumber")]) == (24, 4):
                        break
                else:
                    raise RuntimeError("Could not reach New Bark Town")
                emulator.tick(240)
                emulator.memory[addr("wOptions")] = 1  # fast text
                if (emulator.memory[addr("wXCoord")], emulator.memory[addr("wYCoord")]) != (9, 10):
                    raise RuntimeError("Unexpected spawn coordinate")

                oracle = subprocess.Popen(
                    [str(args.oracle.resolve())],
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    text=True,
                )
                link = {"frame": bytes(640), "ack": 0}

                def request(_):
                    command = emulator.register_file.A
                    oracle.stdin.write(f"{command}\n")
                    oracle.stdin.flush()
                    response = oracle.stdout.readline()
                    if not response:
                        raise RuntimeError("Oracle stopped responding")
                    link["frame"] = bytes.fromhex(response).ljust(640, b"\0")
                    link["ack"] = (emulator.memory[addr("wSeedTicket")] + 1) % 256 or 1

                def page(_):
                    index = emulator.memory[addr("wSeedPage")]
                    data = (
                        bytes([2, link["ack"], 0, index])
                        + link["frame"][index * 32 : index * 32 + 32]
                        + bytes([1, 2])
                    )
                    bank, pc = symbols["SeedLinkPage"]
                    emulator.memory[bank, pc : pc + 38] = data

                emulator.hook_register(*symbols["SeedSend"], request, None)
                emulator.hook_register(*symbols["SeedReadPage"], page, None)
                encoder = subprocess.Popen(
                    [
                        "ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
                        "-f", "rawvideo", "-pixel_format", "rgb24",
                        "-video_size", "160x144", "-framerate", "30",
                        "-i", "-", "-vf", "scale=640:576:flags=neighbor",
                        "-c:v", "libx264", "-pix_fmt", "yuv420p",
                        "-crf", "18", "-movflags", "+faststart",
                        str(args.output),
                    ],
                    stdin=subprocess.PIPE,
                )
                frame_number = 0

                def advance(count):
                    nonlocal frame_number
                    for _ in range(count):
                        emulator.tick(1)
                        frame_number += 1
                        if frame_number % 2 == 0:
                            encoder.stdin.write(emulator.screen.image.convert("RGB").tobytes())

                def tap(key, pause=100):
                    emulator.button_press(key)
                    advance(2)
                    emulator.button_release(key)
                    advance(pause)

                def walk(key, expected):
                    start = (emulator.memory[addr("wXCoord")], emulator.memory[addr("wYCoord")])
                    emulator.button_press(key)
                    for _ in range(120):
                        advance(1)
                        current = (emulator.memory[addr("wXCoord")], emulator.memory[addr("wYCoord")])
                        if current != start:
                            break
                    emulator.button_release(key)
                    advance(45)
                    if current != expected:
                        raise RuntimeError(f"Walked to {current}, expected {expected}")

                advance(90)
                walk("left", (8, 10))
                walk("up", (8, 9))
                advance(45)
                tap("a", 180)  # ordinary town sign dialogue
                for _ in range(3):
                    tap("b")
                tap("a", 180)  # second interaction opens the signer
                tilemap = bytes(emulator.memory[addr("wTilemap") : addr("wTilemap") + 360])
                # Reject a valid video that silently ends in the overworld.
                letters = {**{0x80 + i: chr(65 + i) for i in range(26)},
                           **{0xA0 + i: chr(97 + i) for i in range(26)},
                           0x7F: " "}
                text = "".join(letters.get(value, ".") for value in tilemap)
                if "Recover seed" not in text:
                    raise RuntimeError("Signer menu did not open")
                advance(150)

                encoder.stdin.close()
                if encoder.wait() != 0:
                    raise RuntimeError("ffmpeg failed")
                encoder = None
                print(args.output)
            finally:
                if encoder is not None:
                    encoder.stdin.close()
                    encoder.wait()
                if oracle is not None:
                    oracle.terminate()
                    oracle.wait()
                emulator.stop(save=False)


if __name__ == "__main__":
    main()
