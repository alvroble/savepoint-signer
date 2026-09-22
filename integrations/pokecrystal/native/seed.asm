; Shared Terminal frontend. All cryptography/SD stay on RP2350.
SECTION "Crystal seed status", ROM0
SeedLinkMarker::
 db $43,$53,$44,$32,1
SeedLinkPage:: ds 38

SECTION "Crystal seed local", WRAM0
wSeedStage: ds 38
SECTION "Crystal seed controls", WRAM0
wSeedTicket: db
wSeedKey: db
wSeedFocus: db
wSeedMenu: db
wSeedView: db
wSeedNetwork: db
wSeedPage: db
wSeedFailed: db
wSeedCharPage: db
wSeedJoy: db
wSeedLCDC:: db
wSeedBusy:: db

SECTION "Crystal seed private", WRAMX, BANK[2]
wSeedReport: ds 640
wSeedQR: ds 2704
wSeedEcho:: ds 21
wSeedQRHint: ds 96
wSeedPrivateEnd::

SECTION "Crystal seed UI", ROMX
CrystalSeed::
 ldh a,[hInMenu]
 push af
 ld a,TRUE
 ldh [hInMenu],a
 xor a
 ldh [hBGMapMode],a
 ldh [rVBK],a
 call ClearWindowData
 farcall ReanchorBGMap_NoOAMUpdate
 call LoadStandardMenuHeader
 call ClearSprites
 call DisableSpriteUpdates
 ld a,$90
 ldh [hWY],a
 ldh [rWY],a
 ldh a,[rLCDC]
 ld [wSeedLCDC],a
 farcall SeedEchoClear
 xor a
 ld [wSeedMenu],a
 ld [wSeedView],a
 ld [wSeedKey],a
 ld [wSeedFocus],a
 ld [wSeedCharPage],a
 ld [wSeedBusy],a
 ld a,1
 ld [wSeedNetwork],a
 ld a,10
 call SeedSend
 jp nc,.start
 ; No session has been opened by this UI. Safe to leave if handshake fails.
 call SeedBlank
 hlcoord 1,5
 ld de,.Missing
 call PlaceString
 call SeedShow
 call SeedRelease
.waitMissing
 call DelayFrame
 ldh a,[hJoypadPressed]
 and PAD_B
 jp z,.waitMissing
 jp .leave
.start
 call SeedDraw
 call SeedRelease
.loop
 call DelayFrame
 ldh a,[hJoypadPressed]
 and a
 jp z,.loop
 ld [wSeedJoy],a
 call SeedInput
 jp c,.leave
 call SeedDraw
 jp .loop
.leave
 call SeedRelease
 ; Clear only reserved private bytes; never game SRAM or bank 1.
 di
 ldh a,[rWBK]
 ld b,a
 ld a,2
 ldh [rWBK],a
 ld hl,wSeedReport
 ld de,wSeedPrivateEnd - wSeedReport
 xor a
.wipe
 ld [hli],a
 dec de
 ld a,d
 or e
 jp z,.wiped
 xor a
 jp .wipe
.wiped
 ld a,b
 ldh [rWBK],a
 ei
 ld hl,wSeedStage
 ld bc,38
 xor a
 call ByteFill
 call SeedReturn
 pop af
 ldh [hInMenu],a
 ret
.Missing
 db "Signer unavailable", $4e, "B: Return@"

; Start-menu close, then the map palette set. No white fade.
SeedReturn::
 xor a
 ldh [rVBK],a
 ldh [hBGMapMode],a
 ld a,$90
 ldh [hWY],a
 ldh [rWY],a
 ld a,[wSeedLCDC]
 ldh [rLCDC],a
 call ExitMenu
 call CloseText
 ld b,SCGB_MAPPALS
 call GetSGBLayout
 farcall LoadOW_BGPal7
 call UpdateTimePals
 call EnableSpriteUpdates
 xor a
 ldh [rVBK],a
 ldh [hBGMapMode],a
 ret

SeedRelease:
 call DelayFrame
 ldh a,[hJoypadDown]
 and a
 jp nz,SeedRelease
 ret

; Read one reserved bank-2 byte; native engine always sees original WRAM bank.
SeedByte:
 push bc
 di
 ldh a,[rWBK]
 ld b,a
 ld a,2
 ldh [rWBK],a
 ld a,[hl]
 ld c,a
 ld a,b
 ldh [rWBK],a
 ld a,c
 ei
 pop bc
 ret

; Transport response: copy then validate commit sequence, version and page.
SeedReadPage:
 ld a,[wSeedPage]
 ld [$7002],a
 call DelayFrame
 ld a,[SeedLinkPage]
 bit 0,a
 jp nz,.bad
 push af
 ld hl,SeedLinkPage
 ld de,wSeedStage
 ld bc,38
 call CopyBytes
 pop af
 ld b,a
 ld a,[SeedLinkPage]
 cp b
 jp nz,.bad
 ld a,[wSeedStage+37]
 cp b
 jp nz,.bad
 ld a,[wSeedStage+36]
 cp 1
 jp nz,.bad
 ld a,[wSeedPage]
 ld b,a
 ld a,[wSeedStage+3]
 cp b
 jp nz,.bad
 and a
 ret
.bad
 scf
 ret

SeedSend:
 ld [$7000],a
 xor a
 ld [wSeedFailed],a
 ld [wSeedPage],a
 ld a,[wSeedTicket]
 inc a
 jp nz,.ticket
 inc a
.ticket
 ld [wSeedTicket],a
 ld [$7001],a
 ld bc,1200
.wait
 push bc
 call SeedReadPage
 pop bc
 jp c,.again
 ld a,[wSeedStage+1]
 ld d,a
 ld a,[wSeedTicket]
 cp d
 jp nz,.again
 ld a,[wSeedStage+2]
 and a
 jp z,.fetch
.again
 ld a,[wSeedBusy]
 and a
 jp z,.countdown
 ld a,c
 and 3
 add '0'
 ld [wTilemap + 10 * SCREEN_WIDTH + 9],a
.countdown
 dec bc
 ld a,b
 or c
 jp nz,.wait
 ld a,1
 ld [wSeedFailed],a
 scf
 ret
.fetch
 xor a
 ld [wSeedPage],a
.next
 call SeedReadPage
 jp c,.againFetch
 ; Freeze this response in private WRAM; page addresses are bounded.
 ld a,[wSeedPage]
 ld l,a
 ld h,0
 REPT 5
 add hl,hl
 ENDR
 ld de,wSeedReport
 add hl,de
 ld d,h
 ld e,l
 di
 ldh a,[rWBK]
 push af
 ld a,2
 ldh [rWBK],a
 ld hl,wSeedStage+4
 ld bc,32
 call CopyBytes
 pop af
 ldh [rWBK],a
 ei
 ld hl,wSeedPage
 inc [hl]
 ld a,[hl]
 cp 4
 jp c,.next
 ld hl,wSeedReport+1
 call SeedByte
 cp 7
 jp nz,.done
 ld a,[wSeedPage]
 cp 11
 jp c,.next
.done
 and a
 ret
.againFetch
 ; A coherent page failed; expose failure, never act on partial report.
 ld a,1
 ld [wSeedFailed],a
 scf
 ret

SeedBlank:
 xor a
 ldh [hBGMapMode],a
 hlcoord 0,0
 ld a,' '
 ld bc,SCREEN_AREA
 call ByteFill
 ret
SeedShow:
 ld a,$90
 ldh [hWY],a
 ldh [rWY],a
 call WaitBGMap2
 ld b,SCGB_DIPLOMA
 call GetSGBLayout
 call SetDefaultBGPAndOBP
 xor a
 ldh [hBGMapMode],a
 ret
; Preserve the destination pointer used by PlaceString and keyboard rendering.
SeedState:
 push hl
 ld hl,wSeedReport+1
 call SeedByte
 pop hl
 ret

; ASCII conversion keeps address case intact. Unsupported display glyphs '?'.
SeedChar:
 push bc
 call .convert
 pop bc
 ret
.convert
 cp $61
 jp c,.upper
 cp $7b
 jp nc,.unknown
 add 'a'-$61
 ret
.upper
 cp $41
 jp c,.number
 cp $5b
 jp nc,.punct
 add 'A'-$41
 ret
.number
 cp $30
 jp c,.punct
 cp $3a
 jp nc,.punct
 add '0'-$30
 ret
.punct
 cp $20
 ld b,' '
 jp z,.use
 cp $21
 ld b,'!'
 jp z,.use
 cp $26
 ld b,'&'
 jp z,.use
 cp $27
 ld b,"'"
 jp z,.use
 cp $28
 ld b,'('
 jp z,.use
 cp $29
 ld b,')'
 jp z,.use
 cp $2c
 ld b,','
 jp z,.use
 cp $2d
 ld b,'-'
 jp z,.use
 cp $2e
 ld b,'.'
 jp z,.use
 cp $2f
 ld b,'/'
 jp z,.use
 cp $3a
 ld b,':'
 jp z,.use
 cp $3b
 ld b,';'
 jp z,.use
 cp $3f
 ld b,'?'
 jp z,.use
 cp $5b
 ld b,'['
 jp z,.use
 cp $5d
 ld b,']'
 jp z,.use
 jp .unknown
.use
 ld a,b
 ret
.unknown
 ld a,'?'
 ret
; HL private source, DE tilemap destination, C maximum bytes (linear wrapping).
SeedField:
.loop
 call SeedByte
 and a
 ret z
 call SeedChar
 ld [de],a
 inc hl
 inc de
 dec c
 jp nz,.loop
 ret
SeedNumber:
 ; A 0..255; output to DE, two/three digits without library scratch.
 ld b,0
.hund
 cp 100
 jp c,.tens
 sub 100
 inc b
 jp .hund
.tens
 ld c,a
 ld a,b
 and a
 jp z,.nohund
 add '0'
 ld [de],a
 inc de
.nohund
 ld a,c
 ld b,'0'
.ten
 cp 10
 jp c,.one
 sub 10
 inc b
 jp .ten
.one
 ld c,a
 ld a,b
 ld [de],a
 inc de
 ld a,c
 add '0'
 ld [de],a
 ret

SeedDraw:
 xor a
 ld [wSeedBusy],a
 call SeedBlank
 ld a,[wSeedFailed]
 and a
 jp nz,SeedDrawFailure
 ld a,[wSeedView]
 and a
 jp z,SeedDrawMenu
 cp 1
 jp z,SeedDrawChoose
 call SeedState
 cp 1
 jp z,SeedDrawKeyboard
 cp 2
 jp z,SeedDrawKeyboard
 cp 4
 jp z,SeedDrawIdentity
 cp 5
 jp z,SeedDrawIdentity
 cp 7
 jp nz,.notQR
 farcall SeedDrawQR
 ret
.notQR
 cp 6
 jp z,SeedDrawInvalid
 cp 17
 jp z,SeedDrawFiles
 jp SeedDrawReview

SeedDrawMenu:
 hlcoord 7,0
 ld de,.Title
 call PlaceString
 hlcoord 1,2
 call SeedState
 cp 5
 ld de,.Locked
 jp nz,.status
 ld hl,wSeedReport+6
 call SeedByte
 and a
 ld de,.TestOpen
 jp z,.open
 ld de,.MainOpen
.open
 hlcoord 1,2
.status
 call PlaceString
 hlcoord 0,4
 lb bc,11,18
 call TextboxBorder
 hlcoord 3,5
 ld de,.Items
 call PlaceString
 ld a,[wSeedMenu]
 add a
 ld b,a
 hlcoord 1,5
 ld de,SCREEN_WIDTH
.mark
 ld a,b
 and a
 jp z,.cursor
 add hl,de
 dec b
 jp .mark
.cursor
 ld [hl],'▶'
 hlcoord 1,27
 ld de,.Hint
 call PlaceString
 jp SeedShow
.Title db "SIGNER@"
.Locked db "NO SEED LOADED@"
.MainOpen db "MAINNET     OPEN@"
.TestOpen db "TESTNET     OPEN@"
; NEXT advances two rows in Crystal: exactly one per menu item.
.Items db "Recover seed",$4e,"Receive address",$4e,"Export zpub/vpub",$4e,"Export xpub/tpub",$4e,"Sign PSBT",$4e,"Lock and return@"
.Hint db "A Select  B Lock@"

SeedDrawChoose:
 hlcoord 3,0
 ld de,.Title
 call PlaceString
 hlcoord 1,3
 ld de,.NetworkLabel
 call PlaceString
 hlcoord 2,5
 ld a,[wSeedNetwork]
 and a
 ld de,.Main
 jp nz,.net
 ld de,.Test
.net
 call PlaceString
 hlcoord 0,7
 lb bc,4,18
 call TextboxBorder
 hlcoord 3,8
 ld de,.Words
 call PlaceString
 hlcoord 1,8
 ld a,[wSeedFocus]
 and a
 jp z,.cursor
 hlcoord 1,10
.cursor
 ld [hl],'▶'
 hlcoord 1,14
 ld de,.NetworkHint
 call PlaceString
 hlcoord 1,17
 ld de,.Hint
 call PlaceString
 jp SeedShow
.NetworkLabel db "Network@"
.NetworkHint db "LEFT/RIGHT Network@"
.Title db "RECOVER SEED@"
.Main db "( MAINNET )@"
.Test db "( TESTNET )@"
.Words db "12 words",$4e,"24 words@"
.Hint db "A Next  B Back@"

SeedDrawKeyboard:
 call SeedState
 cp 2
 jp z,.passHead
 hlcoord 0,0
 ld de,.Word
 jp .heading
.passHead
 hlcoord 0,0
 ld de,.Pass
.heading
 call PlaceString
 call SeedState
 cp 2
 jp z,.pass
 ld hl,wSeedReport+3
 call SeedByte
 inc a
 decoord 5,0
 call SeedNumber
 hlcoord 8,0
 ld [hl],'/'
 ld hl,wSeedReport+2
 call SeedByte
 decoord 10,0
 call SeedNumber
 ld hl,wSeedReport+10
 decoord 8,1
 ld c,4
 call SeedField
 ld hl,wSeedReport+16
 decoord 2,3
 ld c,8
 call SeedField
 ld hl,wSeedReport+25
 decoord 2,4
 ld c,8
 call SeedField
 ld hl,wSeedReport+34
 decoord 2,5
 ld c,8
 call SeedField
 ld a,[wSeedFocus]
 and a
 jp z,.grid
 hlcoord 0,3
 ld [hl],"▶"
 jp .grid
.pass
 ld hl,wSeedReport+5
 call SeedByte
 decoord 14,0
 call SeedNumber
 farcall SeedDrawEcho
.grid
 ld b,0
.loop
 push bc
 ld a,b
 call SeedCell
 ld a,b
 call SeedKeyChar
 call SeedChar
 ld [hl],a
 ld a,[wSeedKey]
 cp b
 jp nz,.unselected
 dec hl
 ld [hl],'['
 inc hl
 inc hl
 ld [hl],']'
.unselected
 pop bc
 inc b
 ld a,b
 cp 30
 jp c,.loop
 hlcoord 0,14
 ld de,.Keys
 call PlaceString
 hlcoord 0,16
 call SeedState
 cp 2
 ld de,.PassHint
 jp z,.hint
 ld de,.WordHint
.hint
 call PlaceString
 jp SeedShow
.Word db "WORD@"
.Pass db "PASSPHRASE@"
.Keys db "A Type B Delete@"
.WordHint db "SEL List START Lock@"
.PassHint db "SEL Page START Done@"

; Input index A -> HL cell center; B preserved.
SeedCell:
 push bc
 ld c,0
.row
 cp 5
 jp c,.col
 sub 5
 inc c
 jp .row
.col
 add a
 add a
 inc a
 ld e,a
 ld d,0
 hlcoord 0,7
 add hl,de
 ld de,20
.rows
 ld a,c
 and a
 jp z,.done
 add hl,de
 dec c
 jp .rows
.done
 pop bc
 ret
SeedKeyChar:
 push hl
 push bc
 ld c,a
 call SeedState
 cp 2
 ld a,c
 jp z,.pass
 cp 26
 jp nc,.space
 add $61
 pop bc
 pop hl
 ret
.pass
 ld b,a
 ld a,[wSeedCharPage]
 and a
 ld a,b
 jp nz,.later
 cp 26
 jp nc,.space
 add $61
 pop bc
 pop hl
 ret
.space
 ld a,$20
 pop bc
 pop hl
 ret
.later
 ld c,a
 ld a,[wSeedCharPage]
 dec a
 jp z,.ascii
 ld b,a
 ld a,c
.add30
 add 30
 dec b
 jp nz,.add30
 ld c,a
.ascii
 ld a,c
 add 32
 pop bc
 pop hl
 ret

SeedDrawIdentity:
 hlcoord 2,0
 call SeedState
 cp 4
 ld de,.Title
 jp z,.title
 ld de,.Receive
.title
 call PlaceString
 hlcoord 0,2
 ld de,.Fingerprint
 call PlaceString
 ld hl,wSeedReport+48
 decoord 0,3
 ld c,8
 call SeedField
 ld hl,wSeedReport+6
 call SeedByte
 and a
 ld de,.TestAddress
 jp z,.network
 ld de,.MainAddress
.network
 hlcoord 0,5
 call PlaceString
 ld hl,wSeedReport+64
 decoord 0,7
 ld c,44
 call SeedField
 hlcoord 0,12
 ld de,.Compare
 call PlaceString
 hlcoord 1,17
 call SeedState
 cp 4
 ld de,.Confirm
 jp z,.hint
 ld de,.Next
.hint
 call PlaceString
 jp SeedShow
.Title db "VERIFY KEYS@"
.Receive db "RECEIVE ADDRESS@"
.Fingerprint db "Fingerprint@"
.MainAddress db "Mainnet address@"
.TestAddress db "Testnet address@"
.Compare db "Compare with your",$4e,"coordinator app.@"
.Confirm db "A Confirm  B Lock@"
.Next db "A Next addr B Back@"

SeedDrawInvalid:
 hlcoord 0,6
 ld de,.Error
 call PlaceString
 jp SeedShow
.Error db "Invalid mnemonic",$4e,"B Edit SELECT Lock@"
SeedDrawFailure:
 hlcoord 0,5
 ld de,.Error
 call PlaceString
 jp SeedShow
.Error db "Cart timed out.",$4e,"B: retry LOCK",$4e,"Do not enter words.@"

SeedDrawFiles:
 hlcoord 4,0
 ld de,.Title
 call PlaceString
 hlcoord 1,1
 ld de,.File
 call PlaceString
 ld hl,wSeedReport+8
 call SeedByte
 inc a
 decoord 6,1
 call SeedNumber
 hlcoord 9,1
 ld [hl],'/'
 ld hl,wSeedReport+9
 call SeedByte
 decoord 11,1
 call SeedNumber
 ld hl,wSeedReport+16
 decoord 0,4
 ld c,60
 call SeedField
 hlcoord 1,8
 ld de,.Size
 call PlaceString
 ld hl,wSeedReport+96
 decoord 1,10
 ld c,12
 call SeedField
 ld h,d
 ld l,e
 ld de,.Bytes
 call PlaceString
 hlcoord 1,13
 ld de,.Hint
 call PlaceString
 jp SeedShow
.Title db "SELECT PSBT@"
.File db "File@"
.Size db "File size@"
.Bytes db " bytes@"
.Hint db "UP/DOWN File",$4e,"LEFT/RIGHT Name",$4e,"A Open  B Cancel@"

SeedDrawReview:
 hlcoord 1,0
 ld hl,wSeedReport+6
 call SeedByte
 ld de,.Main
 and a
 jp nz,.net
 ld de,.Test
.net
 hlcoord 1,0
 call PlaceString
 ld hl,wSeedReport+16
 decoord 0,3
 ld c,20
 call SeedField
 ld hl,wSeedReport+36
 decoord 0,5
 ld c,20
 call SeedField
 call SeedState
 cp 10
 jp nz,.error
 ld hl,wSeedReport+10
 call SeedByte
 and a
 jp nz,.approve
 ld hl,wSeedReport+64
 decoord 0,7
 ld c,44
 call SeedField
 hlcoord 0,14
 ld de,.Pages
 jp .hint
.approve
 hlcoord 0,14
 ld de,.Sign
 jp .hint
.error
 ld hl,wSeedReport+16
 decoord 0,3
 ld c,48
 call SeedField
 hlcoord 0,14
 ld de,.Back
.hint
 call PlaceString
 jp SeedShow
.Main db "MAINNET TRANSACTION@"
.Test db "TESTNET TRANSACTION@"
.Pages db "LEFT/RIGHT Review",$4e,"B Cancel@"
.Sign db "A SIGN AND SAVE",$4e,"B Cancel@"
.Back db "B Back@"

SeedInput:
 ld a,[wSeedFailed]
 and a
 jp z,.normal
 ld a,[wSeedJoy]
 and PAD_B
 jp z,.stay
 jp SeedLockExit
.normal
 ld a,[wSeedView]
 and a
 jp z,SeedMenuInput
 cp 1
 jp z,SeedChooseInput
 call SeedState
 cp 1
 jp z,SeedKeyboardInput
 cp 2
 jp z,SeedKeyboardInput
 cp 4
 jp z,.confirm
 cp 5
 jp z,.receive
 cp 7
 jp z,.qr
 cp 6
 jp z,.invalid
 cp 17
 jp z,SeedFileInput
 cp 10
 jp z,SeedReviewInput
 ld a,[wSeedJoy]
 and PAD_B
 jp z,.stay
 ld a,21
 jp .backCommand
.confirm
 ld a,[wSeedJoy]
 bit B_PAD_B,a
 jp nz,SeedLockExit
 and PAD_A
 jp z,.stay
 ld a,9
 jp .backCommand
.receive
 ld a,[wSeedJoy]
 bit B_PAD_B,a
 jp nz,.menu
 and PAD_A
 jp z,.stay
 ld a,11
 jp SeedCommand
.qr
 ld a,[wSeedJoy]
 and PAD_B
 jp z,.stay
 call DisableLCD
 xor a
 ldh [rVBK],a
 call LoadTilesetGFX
 call LoadStandardFont
 call LoadFontsExtra
 ld a,[wSeedLCDC]
 ldh [rLCDC],a
 call EnableLCD
 ld a,13
 jp .backCommand
.invalid
 ld a,[wSeedJoy]
 bit B_PAD_SELECT,a
 jp nz,SeedLockExit
 and PAD_B
 jp z,.stay
 ld a,7
 jp SeedCommand
.backCommand
 call SeedSend
.menu
 xor a
 ld [wSeedView],a
.stay
 and a
 ret
SeedCommand:
 cp 8
 call z,SeedWorking
 call SeedSend
 and a
 ret
; Rendering precedes the potentially slow seed derivation. Preserve command A.
SeedWorking:
 push af
 call SeedBlank
 hlcoord 1,5
 ld de,.Text
 call PlaceString
 call SeedShow
 ld a,1
 ld [wSeedBusy],a
 ldh [hBGMapMode],a
 pop af
 ret
.Text db "Deriving keys", $4e, "Please wait...@"

SeedLockExit:
 ld a,10
 call SeedSend
 jp c,.failed
 scf
 ret
.failed
 and a
 ret

SeedMenuInput:
 ld a,[wSeedJoy]
 bit B_PAD_B,a
 jp nz,SeedLockExit
 bit B_PAD_UP,a
 jp nz,.up
 bit B_PAD_DOWN,a
 jp nz,.down
 and PAD_A
 jp z,.stay
 ld a,[wSeedMenu]
 and a
 jp z,.recover
 cp 5
 jp z,SeedLockExit
 ld b,a
 call SeedState
 cp 5
 jp nz,.recover
 ld a,2
 ld [wSeedView],a
 ld a,b
 cp 1
 jp z,.stay
 cp 2
 ld a,16
 jp z,.send
 ld a,b
 cp 3
 ld a,12
 jp z,.send
 ld a,17
.send
 jp SeedCommand
.recover
 ld a,1
 ld [wSeedView],a
 xor a
 ld [wSeedFocus],a
 jp .stay
.up
 ld a,[wSeedMenu]
 and a
 jp nz,.dec
 ld a,6
.dec
 dec a
 jp .store
.down
 ld a,[wSeedMenu]
 inc a
 cp 6
 jp c,.store
 xor a
.store
 ld [wSeedMenu],a
.stay
 and a
 ret

SeedChooseInput:
 ld a,[wSeedJoy]
 bit B_PAD_B,a
 jp nz,.back
 and PAD_LEFT | PAD_RIGHT
 jp nz,.network
 ld a,[wSeedJoy]
 and PAD_UP | PAD_DOWN
 jp nz,.count
 ld a,[wSeedJoy]
 and PAD_A
 jp z,.stay
 ld a,[wSeedFocus]
 inc a
 ld b,a
 ld a,[wSeedNetwork]
 and a
 ld a,b
 jp z,.begin
 add 13
.begin
 push af
 ld a,2
 ld [wSeedView],a
 xor a
 ld [wSeedKey],a
 ld [wSeedFocus],a
 ld [wSeedCharPage],a
 farcall SeedEchoClear
 pop af
 jp SeedCommand
.back
 xor a
 ld [wSeedView],a
 jp .stay
.network
 ld a,[wSeedNetwork]
 xor 1
 ld [wSeedNetwork],a
 jp .stay
.count
 ld a,[wSeedFocus]
 xor 1
 ld [wSeedFocus],a
.stay
 and a
 ret

SeedKeyboardInput:
 ld a,[wSeedJoy]
 bit B_PAD_START,a
 jp nz,.start
 bit B_PAD_SELECT,a
 jp nz,.select
 bit B_PAD_B,a
 jp nz,.delete
 ld a,[wSeedFocus]
 and a
 jp nz,.list
 ld a,[wSeedJoy]
 bit B_PAD_A,a
 jp nz,.type
 bit B_PAD_LEFT,a
 jp nz,.left
 bit B_PAD_RIGHT,a
 jp nz,.right
 bit B_PAD_UP,a
 jp nz,.up
 bit B_PAD_DOWN,a
 jp nz,.down
 jp .stay
.start
 call SeedState
 cp 2
 jp nz,SeedLockExit
 ld a,8
 jp SeedCommand
.select
 call SeedState
 cp 2
 jp z,.page
 ld a,[wSeedFocus]
 xor 1
 ld [wSeedFocus],a
 jp .stay
.page
 ld a,[wSeedCharPage]
 inc a
 and 3
 ld [wSeedCharPage],a
 jp .stay
.delete
 call SeedState
 cp 2
 jp nz,.wordDelete
 farcall SeedEchoPop
 ld a,3
 jp SeedCommand
.wordDelete
 ld hl,wSeedReport+10
 call SeedByte
 and a
 ld a,3
 jp nz,SeedCommand
 ld a,7
 jp SeedCommand
.list
 ld a,[wSeedJoy]
 bit B_PAD_UP,a
 ld a,5
 jp nz,SeedCommand
 ld a,[wSeedJoy]
 bit B_PAD_DOWN,a
 ld a,4
 jp nz,SeedCommand
 ld a,[wSeedJoy]
 and PAD_A
 jp z,.stay
 xor a
 ld [wSeedFocus],a
 ld [wSeedKey],a
 ld a,6
 jp SeedCommand
.type
 ld a,[wSeedKey]
 call SeedKeyChar
 cp 127
 jp nc,.stay
 push af
 call SeedState
 cp 1
 jp nz,.validChar
 pop af
 cp $7b
 jp nc,.stay
 push af
.validChar
 pop af
 push af
 call SeedState
 cp 2
 jp nz,.sendChar
 pop af
 push af
 call SeedChar
 ld b,a
 farcall SeedEchoPush
.sendChar
 pop af
 sub 32
 add $80
 jp SeedCommand
.left
 ld a,[wSeedKey]
 and a
 jp nz,.dec
 ld a,30
.dec
 dec a
 jp .store
.right
 ld a,[wSeedKey]
 inc a
 cp 30
 jp c,.store
 xor a
 jp .store
.up
 ld a,[wSeedKey]
 cp 5
 jp nc,.sub
 add 30
.sub
 sub 5
 jp .store
.down
 ld a,[wSeedKey]
 add 5
 cp 30
 jp c,.store
 sub 30
.store
 ld [wSeedKey],a
.stay
 and a
 ret

SeedFileInput:
 ld a,[wSeedJoy]
 bit B_PAD_B,a
 jp nz,SeedCancel
 bit B_PAD_A,a
 ld a,24
 jp nz,SeedCommand
 ld a,[wSeedJoy]
 bit B_PAD_UP,a
 ld a,23
 jp nz,SeedCommand
 ld a,[wSeedJoy]
 bit B_PAD_DOWN,a
 ld a,22
 jp nz,SeedCommand
 ld a,[wSeedJoy]
 bit B_PAD_LEFT,a
 ld a,26
 jp nz,SeedCommand
 ld a,[wSeedJoy]
 bit B_PAD_RIGHT,a
 ld a,25
 jp nz,SeedCommand
 and a
 ret
SeedReviewInput:
 ld a,[wSeedJoy]
 bit B_PAD_B,a
 jp nz,SeedCancel
 bit B_PAD_RIGHT,a
 ld a,18
 jp nz,SeedCommand
 ld a,[wSeedJoy]
 bit B_PAD_LEFT,a
 ld a,19
 jp nz,SeedCommand
 ld a,[wSeedJoy]
 and PAD_A
 ret z
 ld hl,wSeedReport+10
 call SeedByte
 and a
 ret z
 ld a,20
 jp SeedCommand
SeedCancel:
 ld a,21
 call SeedSend
 xor a
 ld [wSeedView],a
 ret

SECTION "Crystal seed QR", ROMX

; Public QR only. 2x modules centered. Private staging is wiped on exit.
SeedDrawQR::
 call DisableLCD
 di
 ldh a,[rWBK]
 push af
 ld a,2
 ldh [rWBK],a
 xor a
 ldh [rVBK],a
 ld hl,$8800+('B'-$80)*16
 ld de,wSeedQRHint+0
 ld bc,16
 call CopyBytes
 ld hl,wSeedQRHint+16
 ld bc,16
 xor a
 call ByteFill
 ld hl,$8800+('B'-$80)*16
 ld de,wSeedQRHint+32
 ld bc,16
 call CopyBytes
 ld hl,$8800+('a'-$80)*16
 ld de,wSeedQRHint+48
 ld bc,16
 call CopyBytes
 ld hl,$8800+('c'-$80)*16
 ld de,wSeedQRHint+64
 ld bc,16
 call CopyBytes
 ld hl,$8800+('k'-$80)*16
 ld de,wSeedQRHint+80
 ld bc,16
 call CopyBytes
 ld hl,wSeedQR
 ld bc,2704
 xor a
 call ByteFill
 ld [wSeedKey],a
 ld [wSeedFocus],a
 ld hl,wSeedReport+128
 ld b,1
.bits
 ld a,[hl]
 and b
 push hl
 push bc
 call nz,SeedQRModule
 pop bc
 pop hl
 sla b
 jr nz,.next
 ld b,1
 inc hl
.next
 ld a,[wSeedReport+64]
 ld c,a
 ld a,[wSeedKey]
 inc a
 cp c
 jr c,.x
 xor a
 ld [wSeedKey],a
 ld a,[wSeedFocus]
 inc a
 ld [wSeedFocus],a
 cp c
 jr c,.bits
 jr .upload
.x
 ld [wSeedKey],a
 jr .bits
.upload
 xor a
 ldh [rVBK],a
 ld hl,wSeedQR
 ld de,$8000
 ld bc,2704
 call CopyBytes
 ld hl,$8000+2704
 ld bc,16
 xor a
 call ByteFill
 ld hl,wSeedQRHint
 ld de,$8000+170*16
 ld bc,96
 call CopyBytes
 ld hl,$9800
 ld bc,1024
 ld a,169
 call ByteFill
 ld hl,$9800+3*32+3
 xor a
 ld b,13
.rows
 ld c,13
.cols
 ld [hli],a
 inc a
 dec c
 jr nz,.cols
 ld de,19
 add hl,de
 dec b
 jr nz,.rows
 ld hl,$9800+16*32+7
 ld a,170
 ld b,6
.hint
 ld [hli],a
 inc a
 dec b
 jr nz,.hint
 pop af
 ldh [rWBK],a
 xor a
 ldh [hBGMapMode],a
 ldh [hSCX],a
 ldh [hSCY],a
 ldh [rSCX],a
 ldh [rSCY],a
 ldh a,[rLCDC]
 set 4,a
 res 3,a
 res 5,a
 ldh [rLCDC],a
 ei
 call EnableLCD
 ret

SeedEchoClear::
 di
 ldh a,[rWBK]
 push af
 ld a,2
 ldh [rWBK],a
 xor a
 ld [wSeedEcho],a
 pop af
 ldh [rWBK],a
 ei
 ret

; B = tile to append. Ignored once 20 characters are showing.
SeedEchoPush::
 push hl
 di
 ldh a,[rWBK]
 push af
 push bc
 ld a,2
 ldh [rWBK],a
 ld hl,wSeedEcho
 ld a,[hl]
 cp 20
 jp nc,.full
 inc [hl]
 ld c,a
 ld b,0
 add hl,bc
 inc hl
 pop bc
 ld [hl],b
 jp .stored
.full
 pop bc
.stored
 pop af
 ldh [rWBK],a
 ei
 pop hl
 ret

SeedEchoPop::
 push hl
 di
 ldh a,[rWBK]
 push af
 ld a,2
 ldh [rWBK],a
 ld hl,wSeedEcho
 ld a,[hl]
 and a
 jp z,.empty
 dec [hl]
.empty
 pop af
 ldh [rWBK],a
 ei
 pop hl
 ret

SeedDrawEcho::
 di
 ldh a,[rWBK]
 push af
 ld a,2
 ldh [rWBK],a
 ld hl,wSeedEcho
 ld de,wSeedStage
 ld bc,21
 call CopyBytes
 pop af
 ldh [rWBK],a
 ei
 ld hl,wSeedStage
 ld a,[hli]
 and a
 ret z
 ld c,a
 decoord 0,1
.one
 ld a,[hli]
 ld [de],a
 inc de
 dec c
 jp nz,.one
 ret
SeedQRModule:
 ; Center any supported QR width, preserving a full white quiet zone.
 ld a,[wSeedReport+64]
 ld c,a
 ld a,56
 sub c
 ld c,a
 ld a,[wSeedKey]
 add a
 add c
 ld [wSeedPage],a
 ld a,[wSeedReport+64]
 ld c,a
 ld a,48
 sub c
 ld c,a
 ld a,[wSeedFocus]
 add a
 add c
 ld [wSeedCharPage],a
 call SeedQRPixel
 ld hl,wSeedPage
 inc [hl]
 call SeedQRPixel
 ld hl,wSeedCharPage
 inc [hl]
 call SeedQRPixel
 ld hl,wSeedPage
 dec [hl]
SeedQRPixel:
 ld a,[wSeedCharPage]
 srl a
 srl a
 srl a
 ld l,a
 ld h,0
 ld d,h
 ld e,l
 add hl,hl
 add hl,de
 add hl,hl
 add hl,hl
 add hl,de ; tile row * 13
 ld a,[wSeedPage]
 srl a
 srl a
 srl a
 ld e,a
 ld d,0
 add hl,de
 REPT 4
 add hl,hl
 ENDR
 ld a,[wSeedCharPage]
 and 7
 add a
 ld e,a
 add hl,de
 ld de,wSeedQR
 add hl,de
 ld a,[wSeedPage]
 and 7
 ld b,a
 ld a,$80
.mask
 inc b
.loop
 dec b
 jr z,.paint
 srl a
 jr .loop
.paint
 ld b,a
 or [hl]
 ld [hli],a
 ld a,b
 or [hl]
 ld [hl],a
 ret
