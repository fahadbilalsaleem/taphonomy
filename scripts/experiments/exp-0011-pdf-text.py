#!/usr/bin/env python3
"""Extract the text drawn by Tj and TJ operators in a PDF's Flate streams.

A minimal reader for NIST's layout document, so that EXP-0011 needs no PDF
tool. It recovers only part of the document's text: words can be split or
joined, and page numbers can run into adjacent figures. Anything compared
with its output must allow for that.
Usage: exp-0011-pdf-text.py PDF > text
"""
import re, sys, zlib

data = open(sys.argv[1], 'rb').read()
out = []
for m in re.finditer(rb'stream\r?\n(.*?)\r?\nendstream', data, re.S):
    try:
        s = zlib.decompress(m.group(1))
    except zlib.error:
        continue
    for line in s.split(b'\n'):
        for t in re.findall(rb'\[(.*?)\]\s*TJ|\(((?:\\.|[^\\)])*)\)\s*Tj', line):
            seg = t[0] or t[1]
            parts = re.findall(rb'\(((?:\\.|[^\\)])*)\)', seg) if t[0] else [seg]
            out.append(b''.join(parts).decode('latin1'))
        if re.search(rb'T\*|Td|TD|ET', line):
            out.append('\n')
sys.stdout.write(''.join(out) + '\n')
