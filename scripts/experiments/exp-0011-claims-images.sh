#!/usr/bin/env bash
# Usage: exp-0011-claims-images.sh WORKDIR
#
# Makes the volumes of EXP-0011 Appendix C with two exFAT writers, prints
# what exp-0011-scan.py finds on each, and scores each against the tags its
# writer put in every block (exp-0011-score-tags.py) and by a pair-by-pair
# check of condition 2 (exp-0011-condition-two.py). Input, none of it
# committed, in WORKDIR/src (the same two archives exp-0011-made-images.sh
# names):
#   relan-1264da3.tar.gz  https://codeload.github.com/relan/exfat/tar.gz/1264da3d59840434c657e2199d9c8248355940a6
#   ff16.zip              https://elm-chan.org/fsw/ff/arc/ff16.zip  (FatFs R0.16)
# Each is checked by digest first. Both are built and run here; neither is
# copied into Taphonomy. FatFs is built with its clock (FF_FS_NORTC 0) and
# a host whose clock starts at 2026-03-01 10:00:00, so its volumes are the
# same on every run. relan's library takes its time and UTC offset from the
# system clock and TZ, so its volumes' digests differ from run to run; the
# figures printed do not. A failure prints a line starting FAIL. An archive
# that does not match its digest, or a FatFs configuration that did not
# take, stops the script with nothing built or run.
set -u
W=${1:-}
[ -n "$W" ] || { echo "usage: exp-0011-claims-images.sh WORKDIR" >&2; exit 2; }
T=$(cd "$(dirname "$0")" && pwd)
S=$W/src
B=$W/claims

unverified=0
while read -r want file; do
    got=$(sha256sum "$S/$file" 2>/dev/null | cut -c1-64)
    if [ "$got" != "$want" ]; then
        echo "FAIL $file is ${got:-missing}, expected $want"
        unverified=1
    fi
done <<'EOF'
f5957b7602d457f503e6b3d5b2291bcd14489425e09fa20d3160c2adbcb96874 relan-1264da3.tar.gz
99f7dc1f7e095356e4a9e3dbe29959090d8b948afe2bbc5441e52fdf4b85449e ff16.zip
EOF
if [ "$unverified" != 0 ]; then
    echo "FAIL an archive is unverified: nothing is built or run"
    exit 1
fi

rm -rf -- "${B:?}"
mkdir -p "$B/bin" "$B/img" "$B/fatfs"
tar -xzf "$S/relan-1264da3.tar.gz" -C "$B" || echo "FAIL unpack relan"
python3 -I -m zipfile -e "$S/ff16.zip" "$B/fatfs" || echo "FAIL unpack FatFs"

# FatFs: exFAT, long names and f_mkfs on; its clock left on (the
# distribution's FF_FS_NORTC 0), so get_fattime below supplies the times.
F=$B/fatfs/source
sed -i -e 's/^#define FF_USE_MKFS\t\t0/#define FF_USE_MKFS\t\t1/' \
    -e 's/^#define FF_CODE_PAGE\t932/#define FF_CODE_PAGE\t437/' \
    -e 's/^#define FF_USE_LFN\t\t0/#define FF_USE_LFN\t\t1/' \
    -e 's/^#define FF_FS_EXFAT\t\t0/#define FF_FS_EXFAT\t\t1/' "$F/ffconf.h"
[ "$(grep -c -e $'^#define FF_USE_MKFS\t\t1' -e $'^#define FF_CODE_PAGE\t437' \
    -e $'^#define FF_USE_LFN\t\t1' -e $'^#define FF_FS_EXFAT\t\t1' \
    -e $'^#define FF_FS_NORTC\t\t0' "$F/ffconf.h")" = 5 ] ||
    { echo "FAIL configure FatFs: nothing more is built or run"; exit 1; }
cat >"$F/host_s4.c" <<'HOST_S4_END'
/* Builds two of EXP-0011 Appendix C's volumes with
   FatFs, built with a clock (FF_FS_NORTC 0), so each file's times differ
   and its UTC offset bytes are FatFs's own (it writes 0).

   Every 512-byte block of every file holds a tag: "TAG <name> <block>",
   padded with '.', so which file wrote a cluster is read from the cluster.

   Usage: host_s4 reuse|orphan IMAGE SIZE_MB CLUSTER_BYTES

   reuse:  six files in o/; three deleted; remount (FatFs then allocates from
           the volume's start, so freed clusters are reused); two new files
           written into the holes and deleted. Each old file is now a
           deleted set whose clusters a later deleted set claims.
   orphan: sub/rival written and deleted; remount; in the root an empty
           short-named file, a long-named file (four entries) that reuses
           rival's clusters, and a tail file; the first two deleted;
           remount; a new empty file whose set needs four entries takes the
           first four free slots, overwriting the long-named file's File
           entry and leaving its Stream Extension orphaned. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "ff.h"
#include "diskio.h"

static FILE *img;
static LBA_t sectors;
static DWORD ticks;

/* 2026-03-01 10:00:00 plus two seconds per call, local time. */
DWORD get_fattime(void)
{
	DWORD s = 10 * 3600 + 2 * ticks++;
	DWORD h = s / 3600, m = (s / 60) % 60, ds = (s % 60) / 2;
	return ((DWORD)(2026 - 1980) << 25) | (3u << 21) | (1u << 16) | (h << 11) | (m << 5) | ds;
}

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

static void die(const char *what, const char *path, FRESULT r)
{
	fprintf(stderr, "%s %s: %d\n", what, path, (int)r);
	exit(1);
}

/* Writes `blocks` tagged 512-byte blocks to a new file. */
static void tagged(const char *path, unsigned blocks)
{
	FIL f;
	UINT done;
	char buf[512];
	FRESULT r = f_open(&f, path, FA_WRITE | FA_CREATE_ALWAYS);
	if (r) die("open", path, r);
	for (unsigned b = 0; b < blocks; b++) {
		memset(buf, '.', sizeof buf);
		int n = snprintf(buf, sizeof buf, "TAG %s %u ", path, b);
		if (n < 0 || n >= (int)sizeof buf) die("tag", path, FR_INVALID_PARAMETER);
		buf[n] = '.';
		if ((r = f_write(&f, buf, sizeof buf, &done)) || done != sizeof buf) die("write", path, r);
	}
	if ((r = f_close(&f))) die("close", path, r);
	printf("WROTE\t%s\t%u\n", path, blocks);
}

static void gone(const char *path)
{
	FRESULT r = f_unlink(path);
	if (r) die("unlink", path, r);
	printf("DELETED\t%s\n", path);
}

static void remount(FATFS *fs)
{
	FRESULT r;
	f_unmount("");
	if ((r = f_mount(fs, "", 1))) die("mount", "", r);
}

int main(int argc, char **argv)
{
	static BYTE work[FF_MAX_SS * 64];
	FATFS fs;
	FRESULT r;
	if (argc != 5) { fprintf(stderr, "usage: host_s4 reuse|orphan IMAGE SIZE_MB CLUSTER_BYTES\n"); return 2; }
	DWORD cluster = (DWORD)strtoul(argv[4], 0, 10);
	unsigned per = cluster / 512;	/* blocks per cluster */
	MKFS_PARM opt = { FM_EXFAT | FM_SFD, 1, 0, 0, cluster };
	sectors = (LBA_t)strtoul(argv[3], 0, 10) * 2048;
	img = fopen(argv[2], "w+b");
	if (!img || fseek(img, (long)sectors * 512 - 1, SEEK_SET) || fputc(0, img) == EOF) return 1;
	if ((r = f_mkfs("", &opt, work, sizeof work))) die("mkfs", argv[2], r);
	if ((r = f_mount(&fs, "", 1))) die("mount", argv[2], r);

	if (!strcmp(argv[1], "reuse")) {
		/* The old files live in o/, so the new files' sets, written in the
		   root, do not reuse their directory slots. */
		char name[32];
		if ((r = f_mkdir("o"))) die("mkdir", "o", r);
		for (int i = 1; i <= 6; i++) {
			snprintf(name, sizeof name, "o/old-%d.bin", i);
			tagged(name, 3 * per);
		}
		gone("o/old-2.bin");
		gone("o/old-4.bin");
		gone("o/old-6.bin");
		remount(&fs);
		tagged("new-1.bin", 3 * per);	/* old-2's hole, exactly */
		tagged("new-2.bin", 5 * per);	/* old-4's hole, then two of old-6's */
		gone("new-1.bin");
		gone("new-2.bin");
	} else if (!strcmp(argv[1], "orphan")) {
		if ((r = f_mkdir("sub"))) die("mkdir", "sub", r);
		tagged("sub/rival.bin", 2 * per);
		gone("sub/rival.bin");
		remount(&fs);
		tagged("a.bin", 0);
		tagged("b-is-a-long-name-of-24.bin", 2 * per);	/* rival's clusters */
		tagged("c-tail.bin", per);
		gone("a.bin");
		gone("b-is-a-long-name-of-24.bin");
		remount(&fs);
		tagged("d-takes-four-slots-x.bin", 0);
	} else {
		fprintf(stderr, "unknown scenario %s\n", argv[1]);
		return 2;
	}
	f_unmount("");
	fclose(img);
	return 0;
}
HOST_S4_END
(cd "$F" && gcc -O1 -o "$B/bin/fatfs-claims" host_s4.c ff.c ffsystem.c ffunicode.c) ||
    echo "FAIL build FatFs host"

flags="-O1 -D_FILE_OFFSET_BITS=64 -D_GNU_SOURCE"
X=$(echo "$B"/exfat-1264da3*)
cat >"$X/host_tz.c" <<'HOST_TZ_END'
/* Builds EXP-0011 Appendix C's offset volume with relan's
   libexfat (1264da3), which records the local UTC offset, valid, from TZ.

   Every 512-byte block holds "TAG <path> <block> " padded with '.'.

   Usage: host_tz IMAGE   (IMAGE already formatted by mkexfatfs)

   Pairs, each an old file in o/ deleted and a new file in the root that
   reuses its clusters, written two seconds or more later and deleted:
     mixed:   new-mixed.bin is created under offset X, as old-mixed was,
              and last modified under offset Y.
     same:    new-same.bin is created and modified under X.
   Only the mixed pair tells "an unorderable pair refuses both" from "a
   set is judged by its rivals' keys against its own creation key". */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include "exfat.h"

static struct exfat ef;

static void die(const char *what, const char *path, int rc)
{
	fprintf(stderr, "%s %s: %d\n", what, path, rc);
	exit(1);
}

/* Waits until the clock has moved on two seconds, so the next time stored
   (in two-second units) is later. */
static void tick(void)
{
	time_t start = time(NULL);
	while (time(NULL) < start + 2)
		;
}

static void zone(const char *tz)
{
	if (setenv("TZ", tz, 1)) die("setenv", tz, -1);
	exfat_tzset();
	printf("TZ\t%s\n", tz);
}

static void tagged(const char *path, unsigned blocks)
{
	struct exfat_node *node;
	char buf[512];
	int rc = exfat_mknod(&ef, path);
	if (rc) die("mknod", path, rc);
	if ((rc = exfat_lookup(&ef, &node, path))) die("lookup", path, rc);
	for (unsigned b = 0; b < blocks; b++) {
		memset(buf, '.', sizeof buf);
		int n = snprintf(buf, sizeof buf, "TAG %s %u ", path + 1, b);
		if (n < 0 || n >= (int)sizeof buf) die("tag", path, -1);
		buf[n] = '.';
		if (exfat_generic_pwrite(&ef, node, buf, sizeof buf, (off_t)b * 512) != (ssize_t)sizeof buf)
			die("write", path, -1);
	}
	if ((rc = exfat_flush_node(&ef, node))) die("flush", path, rc);
	exfat_put_node(&ef, node);
	printf("WROTE\t%s\t%u\n", path, blocks);
}

static void touch(const char *path)
{
	struct exfat_node *node;
	struct timespec tv[2];
	int rc = exfat_lookup(&ef, &node, path);
	if (rc) die("lookup", path, rc);
	clock_gettime(CLOCK_REALTIME, &tv[0]);
	tv[1] = tv[0];
	exfat_utimes(node, tv);
	if ((rc = exfat_flush_node(&ef, node))) die("flush", path, rc);
	exfat_put_node(&ef, node);
	printf("TOUCHED\t%s\n", path);
}

static void gone(const char *path)
{
	struct exfat_node *node;
	int rc = exfat_lookup(&ef, &node, path);
	if (rc) die("lookup", path, rc);
	rc = exfat_unlink(&ef, node);
	exfat_put_node(&ef, node);
	if (rc) die("unlink", path, rc);
	if ((rc = exfat_cleanup_node(&ef, node))) die("cleanup", path, rc);
	printf("DELETED\t%s\n", path);
}

int main(int argc, char **argv)
{
	if (argc != 2) { fprintf(stderr, "usage: host_tz IMAGE\n"); return 2; }
	/* Offsets in POSIX form: "XXA-4" is UTC+4 h, "XXB-5" UTC+5 h. */
	const char *X = "XXA-4", *Y = "XXB-5";
	zone(X);
	if (exfat_mount(&ef, argv[1], "")) die("mount", argv[1], -1);
	int rc = exfat_mkdir(&ef, "/o");
	if (rc) die("mkdir", "/o", rc);
	tagged("/o/old-mixed.bin", 6);
	tagged("/o/old-same.bin", 6);
	gone("/o/old-mixed.bin");
	gone("/o/old-same.bin");
	tick();
	tagged("/new-mixed.bin", 6);
	tagged("/new-same.bin", 6);
	tick();
	touch("/new-same.bin");
	zone(Y);
	touch("/new-mixed.bin");
	gone("/new-mixed.bin");
	gone("/new-same.bin");
	exfat_unmount(&ef);
	return 0;
}
HOST_TZ_END
(cd "$X" &&
    gcc $flags -DPACKAGE='"exfat"' -DVERSION='"1264da3"' -Ilibexfat \
        -o "$B/bin/mkexfatfs" mkfs/*.c libexfat/*.c &&
    gcc $flags -DPACKAGE='"exfat"' -DVERSION='"1264da3"' -Ilibexfat \
        -o "$B/bin/relan-tz" host_tz.c libexfat/*.c) ||
    echo "FAIL build relan"

I=$B/img
for s in reuse orphan; do
    for c in 4096 512; do
        "$B/bin/fatfs-claims" "$s" "$I/fatfs-$s-$c.img" 4 "$c" >/dev/null ||
            echo "FAIL make fatfs-$s-$c.img"
    done
done
for sc in 8:64 1:8; do
    s=${sc%%:*} mb=${sc##*:}
    f=$I/relan-tz-$((s * 512)).img
    truncate -s "${mb}M" "$f"
    "$B/bin/mkexfatfs" -s "$s" "$f" >/dev/null 2>&1 || echo "FAIL format $(basename "$f")"
    "$B/bin/relan-tz" "$f" >/dev/null || echo "FAIL make $(basename "$f")"
done
(cd "$I" && sha256sum fatfs-*.img)

for f in "$I"/*.img; do
    b=$(basename "$f" .img)
    python3 -I "$T/exp-0011-scan.py" "$f" >"$B/$b.tsv" || echo "FAIL scan $b"
    python3 -I "$T/exp-0011-score-tags.py" "$f" "$B/$b.tsv" >"$B/$b.score" || echo "FAIL score $b"
    grep -e '^==' -e '^TALLY' "$B/$b.score"
    grep -h '^# orphan' "$B/$b.tsv" | sed "s/^/$b /"
    # Each set's creation, modification and access offset bytes, as stored.
    awk -F'\t' -v f="$b" '!/^#/ && $1!="path" {c[($3==1 ? "deleted" : "live") " offsets " $17]++}
        END {for (k in c) print f, k, c[k]}' "$B/$b.tsv" | sort
    python3 -I "$T/exp-0011-condition-two.py" "$f" "$B/$b.tsv" | grep '^TALLY' ||
        echo "FAIL condition two $b"
done
