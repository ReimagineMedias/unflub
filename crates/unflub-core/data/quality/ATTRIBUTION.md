# Quality corpora: sources and licences

These files feed the quality gate (`crates/unflub-core/examples/quality.rs`). They were prepared
on 2026-11-17 by an independent quality lab: Gutenberg boilerplate, fenced code, HTML comments,
URLs and link targets removed, paragraphs unwrapped; spelling, case and punctuation untouched.
`provenance.json` lists each source URL with the SHA-256 of the raw download. `typos.tsv` holds
5,000 single-letter typos (1,250 each of adjacent key, transposition, dropped and doubled
letter) generated from these texts with seed 20261117.

| File | Source | Licence |
|---|---|---|
| `gutenberg.txt` | *Pride and Prejudice* by Jane Austen, Project Gutenberg eBook #1342 (via the GITenberg mirror), Project Gutenberg header and licence text removed | Public domain |
| `node.txt` | Node.js documentation, `doc/api/stream.md` | MIT, below |
| `rust.txt` | *The Rust Programming Language*, chapters 4.1, 4.2, 10.1 to 10.3, 13.1, 13.2, 15.1, 15.4, 15.5 and 16.1 to 16.3 | MIT or Apache-2.0, at your option; MIT below |
| `typos.tsv` | Generated from the three texts above | As its source text |

## Node.js documentation

Copyright Node.js contributors. All rights reserved.

## The Rust Programming Language

Copyright (c) 2010 The Rust Project Developers.

## MIT License (applies to both of the above)

Permission is hereby granted, free of charge, to any person obtaining a copy of this software
and associated documentation files (the "Software"), to deal in the Software without
restriction, including without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or
substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
