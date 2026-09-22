"""Crystal seed UI, shared backend, overworld return, and SAVE/CONTINUE regression."""
import argparse, sys, io, subprocess
from pathlib import Path
from pyboy import PyBoy
parser=argparse.ArgumentParser()
parser.add_argument("build",type=Path)
parser.add_argument("output",type=Path)
parser.add_argument("--oracle",type=Path,required=True)
args=parser.parse_args()
args.output.mkdir(parents=True,exist_ok=True)
rom=args.build/"pokecrystal.gbc"
def fail(message): raise AssertionError(message)
symbols = {}
for line in (args.build / "pokecrystal.sym").read_text().splitlines():
    if not line or line.startswith(";") or ":" not in line.split()[0]:
        continue
    location, name = line.split()
    symbols[name] = tuple(int(value, 16) for value in location.split(":"))


# --- Helpers ---------------------------------------------------------------
def address(name):
    return symbols[name][1]


def read(name):
    return emulator.memory[address(name)]


def write(name, value):
    emulator.memory[address(name)] = value


def block(name, size):
    start = address(name)
    return bytes(emulator.memory[start : start + size])


def gfx_bytes(name, length):
    bank, start = symbols[name]
    return bytes(emulator.memory[bank, start : start + length])


def press(key, frames=2):
    emulator.button_press(key)
    emulator.tick(frames)
    emulator.button_release(key)
    emulator.tick(100)


def rendered_text():
    chars = {
        **{0x80 + i: chr(ord("A") + i) for i in range(26)},
        **{0xA0 + i: chr(ord("a") + i) for i in range(26)},
        **{0xF6 + i: str(i) for i in range(10)},
        0x7F: " ",
        0xF3: "/",
    }
    data = block("wTilemap", 20 * 18)
    return "\n".join(
        "".join(chars.get(value, ".") for value in data[row : row + 20])
        for row in range(0, len(data), 20)
    )


def bg_palette():
    index=emulator.memory[0xff68]
    data=[]
    for i in range(64):
        emulator.memory[0xff68]=i
        data.append(emulator.memory[0xff69])
    emulator.memory[0xff68]=index
    return bytes(data)

def visible_attrs():
    base=0x9c00 if emulator.memory[0xff40]&8 else 0x9800
    x=emulator.memory[0xff43]//8; y=emulator.memory[0xff42]//8
    return bytes(emulator.memory[1,base+((y+r)%32)*32+(x+c)%32] for r in range(18) for c in range(20))

def screenshot(name):
    emulator.screen.image.save(args.output / f"{name}.png")


def oam():
    return bytes(emulator.memory[0xFE00 : 0xFEA0])


def sram():
    # PyBoy 2.7 rejects a slice whose endpoint is exactly $c000.
    return [
        bytes(emulator.memory[bank, 0xA000 : 0xBFFF])
        + bytes([emulator.memory[bank, 0xBFFF]])
        for bank in range(4)
    ]


def read_sram(bank):
    # PyBoy 2.7 slice reads exclude the last byte, so append it explicitly.
    return (
        bytes(emulator.memory[bank, 0xA000 : 0xBFFF])
        + bytes([emulator.memory[bank, 0xBFFF]])
    )


def flush_battery():
    # PyBoy's `stop(save=True)` writes to `<rom>.ram`, not the ram_file
    # opened at init. After an in-game SAVE, copy the four SRAM banks
    # out so a fresh emulator process can load them as the battery.
    payload = bytearray()
    for bank in range(4):
        payload.extend(read_sram(bank))
    battery_path.write_bytes(bytes(payload))


def talk():
    press("a")
    emulator.tick(80)


battery_path = args.output / "battery.sav"
if battery_path.exists():
    battery_path.unlink()
# Pre-size the battery file to MBC3's full 32 KiB so PyBoy's MBC3 RTC
# initialisation does not assert on a short read.
battery_path.write_bytes(b"\x00" * 0x8000)
emulator = PyBoy(
    str(rom),
    window="null",
    cgb=True,
    sound_emulated=False,
    log_level="ERROR",
    ram_file=open(battery_path, "rb+"),
)
emulator.set_emulation_speed(0)

try:
    # Emulator-only fixture: skip player setup and Oak, then replace SPAWN_HOME
    # with New Bark Town at (8, 9), immediately below the town sign at (8, 8).
    for routine in ("PlayerProfileSetup", "OakSpeech", "ShrinkPlayer"):
        bank, pc = symbols[routine]
        emulator.memory[bank, pc] = 0xC9  # ret
    bank, pc = symbols["SpawnPoints"]
    emulator.memory[bank, pc : pc + 4] = [24, 4, 8, 9]

    emulator.tick(1000)
    for _ in range(15):
        press("a")
        emulator.tick(120)
        if (read("wMapGroup"), read("wMapNumber")) == (24, 4):
            break
    else:
        raise AssertionError("fixture did not reach New Bark Town")
    emulator.tick(240)
    write("wOptions", 1)  # fast text

    assert (read("wXCoord"), read("wYCoord")) == (8, 9)
    press("up")
    assert (read("wXCoord"), read("wYCoord")) == (8, 9), "sign is not solid"
    screenshot("town-sign")



    oracle=subprocess.Popen([str(args.oracle.resolve())],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
    frame=bytes(640)
    ack=0
    busy_pages=0
    saw_recovery_progress=False
    def request(_):
        global frame,ack,busy_pages
        command=emulator.register_file.A
        if command==8:busy_pages=180
        oracle.stdin.write(str(command)+"\n");oracle.stdin.flush()
        frame=bytes.fromhex(oracle.stdout.readline()).ljust(640,b"\0")
        ack=(read("wSeedTicket")+1)%256 or 1
    def page(_):
        global busy_pages,saw_recovery_progress
        busy=busy_pages>0
        if busy_pages:
            busy_pages-=1
            if busy_pages==1:
                assert "Deriving keys" in rendered_text(),rendered_text()
                screenshot("recovering")
                saw_recovery_progress=True
        n=read("wSeedPage")
        data=bytes([2,ack,int(busy),n])+frame[n*32:n*32+32]+bytes([1,2])
        bank,addr=symbols["SeedLinkPage"]
        emulator.memory[bank,addr:addr+38]=data
    emulator.hook_register(*symbols["SeedSend"],request,None)
    emulator.hook_register(*symbols["SeedReadPage"],page,None)
    def walk_and_attrs():
        frames=[]
        for key,coords in [("down",(8,10)),("right",(9,10)),("left",(8,10)),("up",(8,9))]:
            start=(read("wXCoord"),read("wYCoord"))
            emulator.button_press(key)
            for _ in range(120):
                emulator.tick(1)
                if (read("wXCoord"),read("wYCoord"))!=start: break
            emulator.button_release(key);emulator.tick(80)
            assert (read("wXCoord"),read("wYCoord"))==coords
            frames.append(visible_attrs())
        return frames
    control=io.BytesIO()
    emulator.save_state(control)
    reference_walk=walk_and_attrs()
    control.seek(0);emulator.load_state(control)
    before_sram=sram()
    map_start=address("wOverworldMapAnchor")
    map_end=address("wMapAttributesEnd")
    map_state=bytes(emulator.memory[1,map_start:map_end])
    talk()
    for _ in range(3):press("b")
    talk()
    assert "Recover seed" in rendered_text(),rendered_text()
    labels=["Recover seed","Receive address","Export zpub/vpub","Export xpub/tpub","Sign PSBT","Lock and return"]
    for selected in range(6):
        rows=rendered_text().splitlines()
        for index,label in enumerate(labels):
            assert rows[5+2*index][3:3+len(label)]==label,(index,rows)
            tile=emulator.memory[address("wTilemap")+(5+2*index)*20+1]
            assert (tile!=0x7f)==(index==selected),("cursor",selected,index,tile)
        screenshot("menu-"+str(selected))
        press("down")
    press("a");assert "MAINNET" in rendered_text(),rendered_text()
    press("down")
    assert "24 words" in rendered_text().splitlines()[10]
    assert emulator.memory[address("wTilemap")+10*20+1]!=0x7f
    press("up")
    screenshot("choose")
    press("a");assert frame[1]==1,frame[:16]
    screenshot("keyboard")
    for i in range(12):
        press("select")
        if i==11:
            for _ in range(3):press("down")
        press("a")
    assert frame[1]==2,frame[:16]
    assert bytes(emulator.memory[1,map_start:map_end])==map_state,"seed renderer corrupted overworld metadata"
    assert "START Done" in rendered_text(),rendered_text()
    screenshot("passphrase")
    # The last lower-case keyboard cell is a literal ASCII space, not tile $7f.
    press("left");press("a");assert frame[5]==1
    press("b");assert frame[5]==0
    press("right")
    press("start")
    emulator.tick(300)
    assert saw_recovery_progress
    assert frame[1]==4,frame[:16]
    assert bytes(emulator.memory[1,map_start:map_end])==map_state,"identity renderer corrupted overworld metadata"
    assert "A Confirm" in rendered_text(),rendered_text()
    screenshot("identity")
    assert "bc1" in rendered_text(),rendered_text()
    press("a");assert frame[1]==5
    assert "MAINNET     OPEN" in rendered_text()
    screenshot("menu-open")
    press("down");press("down");press("a")
    assert frame[1]==7,frame[:16]
    screenshot("qr")
    from PIL import Image
    # Every module must match the production QR bitstream, including centering.
    size=frame[64]
    image=emulator.screen.image.convert("RGB")
    x0=80-size;y0=72-size
    for y in range(size):
        for x in range(size):
            pixel=image.getpixel((x0+2*x,y0+2*y))
            actual=sum(pixel)<384
            wanted=bool(frame[128+(y*size+x)//8] & (1<<((y*size+x)%8)))
            assert actual==wanted,("QR module",x,y,pixel,wanted)
    press("b");assert frame[1]==5
    assert "Recover seed" in rendered_text(),rendered_text()
    press("down");press("down");press("a")
    assert frame[1]==17
    assert "01 / 02" in rendered_text(),rendered_text()
    screenshot("files-1")
    press("down");assert "02 / 02" in rendered_text(),rendered_text()
    screenshot("files-2")
    press("up");assert "01 / 02" in rendered_text(),rendered_text()
    press("a");assert frame[1]==10,frame[:16]
    assert "60000 sat" in rendered_text(),rendered_text()
    screenshot("review-output")
    press("right");assert frame[1]==10
    screenshot("review-change")
    press("right");assert frame[1]==10
    screenshot("review-fee")
    press("right");assert frame[10]==1,frame[:16]
    screenshot("review-approve")
    press("b")
    press("b");assert frame[1]==0
    emulator.tick(180)
    assert sram()==before_sram,"seed modified game SRAM"
    bank,start=symbols["wSeedReport"]
    assert not any(emulator.memory[bank,start:address("wSeedPrivateEnd")]),"private UI data remains"
    screenshot("returned")
    assert walk_and_attrs()==reference_walk,"scroll attributes changed after recovery"
    for cycle in range(10):
        talk()
        assert "NEW BARK" in rendered_text()
        for _ in range(3):press("b")
        talk()
        assert "Recover seed" in rendered_text()
        press("b")
        assert walk_and_attrs()==reference_walk,("scroll attributes",cycle)
        assert sram()==before_sram,("game SRAM",cycle)
    # --- In-game SAVE → fresh emulator → CONTINUE -------------------------
    # This is the actual Crystal save path, not a Python-injected SRAM
    # sentinel. The cartridge firmware persists whatever the game writes
    # here; CONTINUE on a power cycle is the acceptance criterion.
    def encode_name(text):
        return bytes([0x80 + ord(ch) - ord("A") for ch in text]) + bytes([0x50])

    player_name = encode_name("TEST")
    start = address("wPlayerName")
    for offset, byte in enumerate(player_name):
        emulator.memory[start + offset] = byte

    saved_coords = (
        read("wMapGroup"),
        read("wMapNumber"),
        read("wXCoord"),
        read("wYCoord"),
    )

    def wait_text(fragment, limit=400, message=None):
        for _ in range(limit):
            if fragment in rendered_text():
                return rendered_text()
            emulator.tick(8)
        fail(message or f"did not see {fragment!r}:\n{rendered_text()}")

    STARTMENUITEM_SAVE = 4
    press("start")
    wait_text("PACK", message="Start menu did not reopen for SAVE")
    emulator.tick(60)
    item_count = read("wMenuItemsList")
    items = [
        emulator.memory[address("wMenuItemsList") + 1 + i] for i in range(item_count)
    ]
    if STARTMENUITEM_SAVE not in items:
        fail(f"SAVE not in start menu items {items}:\n{rendered_text()}")
    save_index = items.index(STARTMENUITEM_SAVE)
    # wMenuCursorPosition is 1-based.
    target = save_index + 1
    for _ in range(12):
        if read("wMenuCursorPosition") == target:
            break
        press("down")
    else:
        fail(
            f"could not move cursor onto SAVE (items={items} target={target} "
            f"cursor={read('wMenuCursorPosition')}):\n{rendered_text()}"
        )
    screenshot("save-cursor")
    emulator.tick(30)
    emulator.button_press("a")
    emulator.tick(12)
    emulator.button_release("a")
    emulator.tick(40)
    for _ in range(80):
        text = rendered_text()
        if "save the game" in text or "Would you like" in text or "SAVING" in text:
            break
        emulator.tick(8)
    else:
        fail(
            f"SAVE confirmation did not open (items={items} target={target} "
            f"cursor={read('wMenuCursorPosition')}):\n{rendered_text()}"
        )
    if "SAVING" not in rendered_text() and "saved" not in rendered_text().lower():
        emulator.button_press("a")
        emulator.tick(12)
        emulator.button_release("a")
        emulator.tick(40)
    wait_text("saved", limit=800, message="in-game SAVE did not finish")
    emulator.tick(120)
    screenshot("saved")
    assert read("wSavedAtLeastOnce") != 0, "wSavedAtLeastOnce was not set"

    flush_battery()
finally:
    emulator.stop(save=False)
    if "oracle" in globals(): oracle.terminate()


def boot_to_continue(battery):
    emu = PyBoy(
        str(rom),
        window="null",
        cgb=True,
        sound_emulated=False,
        log_level="ERROR",
        ram_file=open(battery, "rb+"),
    )
    emu.set_emulation_speed(0)
    return emu


def tap(emu, key, hold=2, gap=30):
    emu.button_press(key)
    emu.tick(hold)
    emu.button_release(key)
    emu.tick(gap)


def title_text(emu):
    chars = {
        **{0x80 + i: chr(ord("A") + i) for i in range(26)},
        **{0xA0 + i: chr(ord("a") + i) for i in range(26)},
        0x7F: " ",
    }
    data = bytes(emu.memory[address("wTilemap") : address("wTilemap") + 20 * 18])
    return "\n".join(
        "".join(chars.get(value, ".") for value in data[row : row + 20])
        for row in range(0, len(data), 20)
    )


def mash_to_continue(emu, limit=12000):
    for i in range(limit):
        text = title_text(emu)
        if "CONTINUE" in text:
            screenshot_from = getattr(emu.screen, "image", None)
            if screenshot_from is not None:
                screenshot_from.save(args.output / "continue.png")
            return text
        if i % 25 == 0:
            tap(emu, "a")
        else:
            emu.tick(1)
    fail(f"CONTINUE did not appear on the title/main menu:\n{title_text(emu)}")


emulator = boot_to_continue(battery_path)
try:
    mash_to_continue(emulator)
    tap(emulator, "a", gap=80)
    tap(emulator, "a", gap=80)
    for _ in range(40):
        tap(emulator, "a", gap=40)
        if (emulator.memory[address("wMapGroup")], emulator.memory[address("wMapNumber")]) == (
            saved_coords[0],
            saved_coords[1],
        ):
            break
    restored_name = bytes(
        emulator.memory[address("wPlayerName") : address("wPlayerName") + len(player_name)]
    )
    assert restored_name == player_name, f"player name after CONTINUE: {restored_name!r}"
    restored_coords = (
        emulator.memory[address("wMapGroup")],
        emulator.memory[address("wMapNumber")],
        emulator.memory[address("wXCoord")],
        emulator.memory[address("wYCoord")],
    )
    assert restored_coords == saved_coords, (
        f"location after CONTINUE {restored_coords} != {saved_coords}"
    )
finally:
    emulator.stop(save=False)

print("PASS: recovery, menu alignment, QR, PSBT review, zeroization, 10 returns, scrolling, SAVE and fresh CONTINUE")
