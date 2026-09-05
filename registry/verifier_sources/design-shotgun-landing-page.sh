#!/usr/bin/env bash
python3 - <<'PYEOF'
from pathlib import Path
from html.parser import HTMLParser
from collections import Counter
import re, sys
class Doc(HTMLParser):
    def __init__(self): super().__init__(); self.text=[]; self.body={}; self.tags=[]; self.hrefs=[]; self.labels=[]
    def handle_starttag(self,t,a):
        d={k.lower():(v or '') for k,v in a}; self.tags.append(t.lower())
        if t.lower()=='body': self.body=d
        if t.lower()=='a': self.hrefs.append(d.get('href',''))
        if d.get('aria-label'): self.labels.append(d['aria-label'])
    def handle_data(self,d): self.text.append(d)
def norm(v): return re.sub(r'\s+',' ',v).strip().lower()
root=Path('tasks/design-shotgun/output'); paths=[root/f'direction-{c}.html' for c in 'abc']; errors=[]; concepts=[]; signatures=[]
for path in paths:
    if not path.is_file(): errors.append(f'missing {path}'); continue
    raw=path.read_text(errors='ignore'); d=Doc(); d.feed(raw); visible=norm(' '.join(d.text)); css=' '.join(re.findall(r'<style[^>]*>(.*?)</style>',raw,re.I|re.S)).lower()
    if len(visible)<350: errors.append(f'{path} is too thin')
    concept=d.body.get('data-direction','').strip().lower(); concepts.append(concept)
    if len(concept)<3: errors.append(f'{path} lacks a named visual concept')
    for required in ['source','calls','tickets','slack','15','trial','credit card']:
        if required not in visible: errors.append(f'{path} misses supplied truth: {required}')
    for pattern in [r'\btrusted by\s+\d',r'\b\d(?:\.\d)?\s*/\s*5\s+stars?',r'\baward[- ]winning\b',r'[“\"]\s*[^”\"]{15,}\s*[”\"]\s*[—-]\s*[A-Z][a-z]+']:
        if re.search(pattern,visible): errors.append(f'{path} contains unsupported proof')
    if '@media' not in css: errors.append(f'{path} lacks responsive behavior')
    counts=Counter(d.tags); signatures.append((counts['section'],counts['aside'],counts['nav'],counts['article'],len(re.findall(r'#[0-9a-f]{6}\b',css))))
if len(set(concepts))!=3: errors.append('directions do not name three distinct concepts')
if len(set(signatures))<2: errors.append('directions lack meaningful structural variation')
cp=root/'comparison.html'
if not cp.is_file(): errors.append('missing comparison.html')
else:
    d=Doc(); d.feed(cp.read_text(errors='ignore')); visible=norm(' '.join(d.text+d.labels))
    for name in ['direction-a.html','direction-b.html','direction-c.html']:
        if name not in [href.split('#',1)[0].split('?',1)[0].rsplit('/',1)[-1] for href in d.hrefs]: errors.append(f'comparison misses link to {name}')
    if not re.search(r'trade[- ]?offs?',visible): errors.append('comparison misses trade-offs')
    if 'risk' not in visible or not re.search(r'best[- ]?fit|audience',visible): errors.append('comparison misses risks or fit guidance')
for e in errors: print('FAIL',e)
sys.exit(0 if not errors else 1)
PYEOF
