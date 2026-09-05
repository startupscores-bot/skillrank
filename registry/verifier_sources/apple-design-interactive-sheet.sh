#!/usr/bin/env bash
python3 - <<'PYEOF'
from pathlib import Path
from html.parser import HTMLParser
import re, sys
class Doc(HTMLParser):
    def __init__(self): super().__init__(); self.text=[]; self.attrs=[]
    def handle_starttag(self,t,a): self.attrs.append((t.lower(),{k.lower():(v or '') for k,v in a}))
    def handle_data(self,d): self.text.append(d)
path=Path('tasks/apple-design/index.html'); raw=path.read_text(errors='ignore') if path.is_file() else ''; low=raw.lower(); d=Doc(); d.feed(raw); visible=re.sub(r'\s+',' ',' '.join(d.text)).lower(); errors=[]
if len(visible)<220: errors.append('implementation is too thin')
for phrase in ['billing export','seven customers','today’s queue']:
    if phrase not in visible: errors.append(f'supplied content changed or disappeared: {phrase}')
for event in ['pointerdown','pointermove','pointerup','keydown']:
    if not re.search(rf'addEventListener\s*\(\s*[\'\"]{event}[\'\"]',raw,re.I): errors.append(f'missing live handler: {event}')
if 'requestanimationframe' not in low or 'cancelanimationframe' not in low: errors.append('animation is not explicitly interruptible')
if not re.search(r'(velocity|speed).{0,500}(threshold|project|target|snap)|(?:threshold|project|target|snap).{0,500}(velocity|speed)',low,re.S): errors.append('settling does not use measured velocity')
if not re.search(r'(spring|stiffness|damping)',low): errors.append('missing spring-like settling model')
if 'prefers-reduced-motion' not in low or not re.search(r'\.matches\b',low): errors.append('reduced-motion preference is not used at runtime')
if not any('aria-expanded' in a for _,a in d.attrs) or not re.search(r'setAttribute\s*\(\s*[\'\"]aria-expanded',raw,re.I): errors.append('expanded state is not initialized and updated')
keys=re.findall(r'[\'\"](Enter|Space|Escape|ArrowUp|ArrowDown|Home|End| )[\'\"]',raw,re.I)
if len(set(keys))<2: errors.append('keyboard controls are incomplete')
if 'touch-action' not in low or 'transform' not in low: errors.append('gesture surface or movement transform is missing')
for e in errors: print('FAIL',e)
sys.exit(0 if not errors else 1)
PYEOF
