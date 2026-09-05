#!/usr/bin/env bash
python3 - <<'PYEOF'
from pathlib import Path
from html.parser import HTMLParser
import re, sys
class Doc(HTMLParser):
    def __init__(self): super().__init__(); self.text=[]; self.body={}; self.tags=[]; self.attrs=[]
    def handle_starttag(self,t,a):
        d={k.lower():(v or '') for k,v in a}; self.tags.append(t.lower()); self.attrs.append((t.lower(),d))
        if t.lower()=='body': self.body=d
    def handle_data(self,d): self.text.append(d)
raw=Path('tasks/frontend-design/index.html').read_text(errors='ignore') if Path('tasks/frontend-design/index.html').is_file() else ''
d=Doc(); d.feed(raw); visible=re.sub(r'\s+',' ',' '.join(d.text)).lower(); css=' '.join(re.findall(r'<style[^>]*>(.*?)</style>',raw,re.I|re.S)).lower(); errors=[]
if len(visible)<280: errors.append('redesign is too thin')
if len(d.body.get('data-visual-concept','').strip())<3: errors.append('missing named visual concept')
for topic,count in [('onboarding friction','18'),('export requests','11'),('mobile access','7')]:
    if topic not in visible or not re.search(rf'\b{count}\b',visible): errors.append(f'missing supplied content: {topic} / {count}')
if re.search(r'font-family\s*:\s*arial\b',css): errors.append('generic starter typography remains')
if ':root' not in css or len(re.findall(r'--[a-z0-9-]+\s*:',css))<6: errors.append('visual system is not tokenized')
for term in ['@media',':focus-visible',':hover']:
    if term not in css: errors.append(f'missing production UI feature: {term}')
for tag in ['main','header']:
    if tag not in d.tags: errors.append(f'missing semantic landmark: {tag}')
if 'nav' not in d.tags and not any('aria-label' in a or 'aria-labelledby' in a for _,a in d.attrs): errors.append('accessible navigation or labeling is missing')
if not any(t in ['button','a','input','select'] for t,_ in d.attrs): errors.append('dashboard has no interactive control')
for pattern in [r'\btrusted by\s+\d',r'\baward[- ]winning\b',r'\b10,?000 teams\b']:
    if re.search(pattern,visible): errors.append('invented product proof detected')
for e in errors: print('FAIL',e)
sys.exit(0 if not errors else 1)
PYEOF
