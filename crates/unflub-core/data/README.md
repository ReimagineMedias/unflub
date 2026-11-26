# Bundled dictionary data

The engine embeds these files with `include_str!` and builds its index from them at startup.

| File | Source | Licence |
|---|---|---|
| `frequency_dictionary_en_82_765.txt` | [SymSpell](https://github.com/wolfgarbe/SymSpell/blob/master/SymSpell/frequency_dictionary_en_82_765.txt) | MIT, Copyright (c) 2018 Wolf Garbe |
| `frequency_bigramdictionary_en_243_342.txt` | [SymSpell](https://github.com/wolfgarbe/SymSpell/blob/master/SymSpell/frequency_bigramdictionary_en_243_342.txt) | MIT, Copyright (c) 2018 Wolf Garbe |
| `extra_words.txt` | unflub: everyday and technical words the SymSpell list lacks, with hand-assigned counts | MIT (this repository) |
| `tech_words.txt` | unflub: about 1,000 programming terms (keywords, standard types, tools, jargon), loaded as known words with a modest count | MIT (this repository) |
| `spelling_variants.txt` | unflub: American and British spellings of the same word; the one the frequency list lacks gets the other's count | MIT (this repository) |
| `misspellings.txt` | unflub: common misspellings the ranking gets wrong, with their fixes; checked before ranking | MIT (this repository) |
| `protected_words.txt` | unflub: chat shorthand, brands, acronyms and lowercase first names that are never corrected or suggested | MIT (this repository) |
| `contractions.txt` | unflub: contractions and their expansions, used to give contractions realistic counts | MIT (this repository) |

The SymSpell files were taken from commit `91fe939e9c1259813ee817e95239629c5badf8ec`
(2026-10-29). We converted them to LF line endings and removed the byte order mark; the
content is otherwise unchanged. The bigram file is 5.1 MB and is kept whole, not pruned.

SymSpell's licence:

```
MIT License

Copyright (c) 2018 Wolf Garbe

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
