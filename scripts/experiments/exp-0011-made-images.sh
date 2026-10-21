#!/usr/bin/env bash
# Usage: exp-0011-made-images.sh WORKDIR
#
# Makes exFAT volumes with three formatters Taphonomy's benchmarks had not
# seen, and prints for each what exp-0011-check-live.py finds and what
# exfatprogs' fsck says. Input, none of it committed, in WORKDIR/src:
#   exfatprogs-8b42748.tar.gz  https://codeload.github.com/exfatprogs/exfatprogs/tar.gz/8b42748
#   relan-1264da3.tar.gz       https://codeload.github.com/relan/exfat/tar.gz/1264da3d59840434c657e2199d9c8248355940a6
#   ff16.zip                   https://elm-chan.org/fsw/ff/arc/ff16.zip  (FatFs R0.16)
# Each is checked by digest first. Every source is built and run here; none
# is copied into Taphonomy. libblkid, which exfatprogs' mkfs uses only to
# warn about an existing signature, is replaced by a stub that finds none:
# every target is a fresh file of zeros. The volumes carry a serial number
# and timestamps from the clock, so their digests differ from run to run;
# the figures printed do not. A failure prints a line starting FAIL. An
# archive that does not match its digest, or a FatFs configuration that
# did not take, stops the script with nothing built or run: it compiles
# and runs the code it unpacks, and must not do so unverified.
set -u
W=${1:-}
[ -n "$W" ] || { echo "usage: exp-0011-made-images.sh WORKDIR" >&2; exit 2; }
T=$(cd "$(dirname "$0")" && pwd)
S=$W/src
B=$W/build

unverified=0
while read -r want file; do
    got=$(sha256sum "$S/$file" 2>/dev/null | cut -c1-64)
    if [ "$got" != "$want" ]; then
        echo "FAIL $file is ${got:-missing}, expected $want"
        unverified=1
    fi
done <<'EOF'
dfe72233e9dea01ad093dd092260359942f19505dbef9276a275c4d0ed33efa6 exfatprogs-8b42748.tar.gz
f5957b7602d457f503e6b3d5b2291bcd14489425e09fa20d3160c2adbcb96874 relan-1264da3.tar.gz
99f7dc1f7e095356e4a9e3dbe29959090d8b948afe2bbc5441e52fdf4b85449e ff16.zip
EOF
if [ "$unverified" != 0 ]; then
    echo "FAIL an archive is unverified: nothing is built or run"
    exit 1
fi

rm -rf -- "${B:?}"
mkdir -p "$B/bin" "$B/img"

tar -xzf "$S/exfatprogs-8b42748.tar.gz" -C "$B" || echo "FAIL unpack exfatprogs"
tar -xzf "$S/relan-1264da3.tar.gz" -C "$B" || echo "FAIL unpack relan"
mkdir -p "$B/fatfs"
python3 -I -m zipfile -e "$S/ff16.zip" "$B/fatfs" || echo "FAIL unpack FatFs"

mkdir -p "$B/stub"
cat >"$B/stub/blkid.h" <<'STUB_END'
#ifndef STUB_BLKID_H
#define STUB_BLKID_H
#include <stddef.h>
typedef void *blkid_cache;
typedef void *blkid_dev;
typedef void *blkid_tag_iterate;
#define BLKID_DEV_CREATE 1
#define BLKID_DEV_VERIFY 2
static inline int blkid_get_cache(blkid_cache *c, const char *f) { (void)f; *c = NULL; return 0; }
static inline blkid_dev blkid_get_dev(blkid_cache c, const char *n, int f) { (void)c; (void)n; (void)f; return NULL; }
static inline blkid_tag_iterate blkid_tag_iterate_begin(blkid_dev d) { (void)d; return NULL; }
static inline int blkid_tag_next(blkid_tag_iterate i, const char **k, const char **v) { (void)i; (void)k; (void)v; return -1; }
static inline void blkid_tag_iterate_end(blkid_tag_iterate i) { (void)i; }
static inline void blkid_put_cache(blkid_cache c) { (void)c; }
static inline int blkid_get_library_version(const char **v, const char **d) { *v = "stub"; *d = "none"; return 0; }
#endif
STUB_END

flags="-O1 -D_FILE_OFFSET_BITS=64 -D_GNU_SOURCE"
(cd "$B"/exfatprogs-8b42748* &&
    gcc $flags -I"$B/stub" -Iinclude -Imkfs -o "$B/bin/mkfs.exfat" mkfs/*.c lib/*.c &&
    gcc $flags -Iinclude -Ifsck -o "$B/bin/fsck.exfat" fsck/*.c lib/*.c) ||
    echo "FAIL build exfatprogs"
(cd "$B"/exfat-1264da3* &&
    gcc $flags -DPACKAGE='"exfat"' -DVERSION='"1264da3"' -Ilibexfat \
        -o "$B/bin/mkexfatfs" mkfs/*.c libexfat/*.c) ||
    echo "FAIL build relan"

# FatFs is a library: the host below formats with f_mkfs and, for
# fatfs-*, writes a.bin, D/b.bin and frag.bin, extended after between.bin
# so that its chain is fragmented.
F=$B/fatfs/source
sed -i -e 's/^#define FF_USE_MKFS\t\t0/#define FF_USE_MKFS\t\t1/' \
    -e 's/^#define FF_CODE_PAGE\t932/#define FF_CODE_PAGE\t437/' \
    -e 's/^#define FF_USE_LFN\t\t0/#define FF_USE_LFN\t\t1/' \
    -e 's/^#define FF_FS_EXFAT\t\t0/#define FF_FS_EXFAT\t\t1/' \
    -e 's/^#define FF_FS_NORTC\t\t0/#define FF_FS_NORTC\t\t1/' "$F/ffconf.h"
[ "$(grep -c -e $'^#define FF_USE_MKFS\t\t1' -e $'^#define FF_CODE_PAGE\t437' \
    -e $'^#define FF_USE_LFN\t\t1' -e $'^#define FF_FS_EXFAT\t\t1' \
    -e $'^#define FF_FS_NORTC\t\t1' "$F/ffconf.h")" = 5 ] ||
    { echo "FAIL configure FatFs: nothing more is built or run"; exit 1; }
cat >"$F/host.c" <<'HOST_END'
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "ff.h"
#include "diskio.h"

static FILE *img;
static LBA_t sectors;

DSTATUS disk_initialize(BYTE pdrv) { (void)pdrv; return 0; }
DSTATUS disk_status(BYTE pdrv) { (void)pdrv; return 0; }
DRESULT disk_read(BYTE pdrv, BYTE *buff, LBA_t sector, UINT count)
{
	(void)pdrv;
	if (fseek(img, (long)sector * 512, SEEK_SET) || fread(buff, 512, count, img) != count)
		return RES_ERROR;
	return RES_OK;
}
DRESULT disk_write(BYTE pdrv, const BYTE *buff, LBA_t sector, UINT count)
{
	(void)pdrv;
	if (fseek(img, (long)sector * 512, SEEK_SET) || fwrite(buff, 512, count, img) != count)
		return RES_ERROR;
	return RES_OK;
}
DRESULT disk_ioctl(BYTE pdrv, BYTE cmd, void *buff)
{
	(void)pdrv;
	switch (cmd) {
	case CTRL_SYNC: fflush(img); return RES_OK;
	case GET_SECTOR_COUNT: *(LBA_t *)buff = sectors; return RES_OK;
	case GET_SECTOR_SIZE: *(WORD *)buff = 512; return RES_OK;
	case GET_BLOCK_SIZE: *(DWORD *)buff = 1; return RES_OK;
	}
	return RES_PARERR;
}

static void fill(const char *path, unsigned bytes, BYTE seed, int append)
{
	FIL f;
	UINT done;
	BYTE buf[4096];
	FRESULT r = f_open(&f, path, append ? (FA_WRITE | FA_OPEN_APPEND) : (FA_WRITE | FA_CREATE_ALWAYS));
	if (r) { fprintf(stderr, "open %s: %d\n", path, r); exit(1); }
	while (bytes) {
		UINT n = bytes < sizeof buf ? bytes : sizeof buf;
		memset(buf, seed++, n);
		if (f_write(&f, buf, n, &done) || done != n) { fprintf(stderr, "write %s\n", path); exit(1); }
		bytes -= n;
	}
	if (f_close(&f)) { fprintf(stderr, "close %s\n", path); exit(1); }
}

int main(int argc, char **argv)
{
	static BYTE work[FF_MAX_SS * 64];
	FATFS fs;
	FRESULT r;
	MKFS_PARM opt = { FM_EXFAT | FM_SFD, 1, 0, 0, (DWORD)strtoul(argv[3], 0, 10) };
	sectors = (LBA_t)strtoul(argv[2], 0, 10) * 2048;
	img = fopen(argv[1], "w+b");
	if (!img || fseek(img, (long)sectors * 512 - 1, SEEK_SET) || fputc(0, img) == EOF) return 1;
	if ((r = f_mkfs("", &opt, work, sizeof work))) { fprintf(stderr, "mkfs: %d\n", r); return 1; }
	if (argc > 4) {
		if ((r = f_mount(&fs, "", 1))) { fprintf(stderr, "mount: %d\n", r); return 1; }
		fill("a.bin", 10000, 1, 0);
		if ((r = f_mkdir("D"))) { fprintf(stderr, "mkdir: %d\n", r); return 1; }
		fill("D/b.bin", 50000, 7, 0);
		fill("frag.bin", 9000, 3, 0);
		fill("between.bin", 9000, 9, 0);
		fill("frag.bin", 20000, 5, 1);
		f_unmount("");
	}
	fclose(img);
	return 0;
}
HOST_END
(cd "$F" && gcc -O1 -o "$B/bin/fatfs-host" host.c ff.c ffsystem.c ffunicode.c) ||
    echo "FAIL build FatFs host"

I=$B/img
format_image() { # name size_MiB command...
    local name=$1 size=$2
    shift 2
    truncate -s "${size}M" "$I/$name"
    "$@" "$I/$name" >/dev/null 2>&1 || echo "FAIL format $name"
}
format_image mkfs-4k.img 64 "$B/bin/mkfs.exfat"
format_image mkfs-512.img 8 "$B/bin/mkfs.exfat" -c 512
format_image relan-4k.img 64 "$B/bin/mkexfatfs" -s 8
format_image relan-512.img 8 "$B/bin/mkexfatfs" -s 1
"$B/bin/fatfs-host" "$I/fatfs-4k.img" 64 4096 files || echo "FAIL format fatfs-4k.img"
"$B/bin/fatfs-host" "$I/fatfs-512.img" 8 512 files || echo "FAIL format fatfs-512.img"

for f in "$I"/*.img; do
    python3 -I "$T/exp-0011-check-live.py" "$f" || echo "FAIL check $(basename "$f")"
    # Run from the image's directory, so fsck names the image and not the
    # work directory, and the output is the same wherever it runs.
    printf '%s\tfsck: %s\n' "$(basename "$f")" \
        "$(cd "$I" && "$B/bin/fsck.exfat" -n "$(basename "$f")" 2>&1 | tail -1)"
done
