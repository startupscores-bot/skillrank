#!/usr/bin/env bash
python3 - <<'PYEOF'
from pathlib import Path
from html.parser import HTMLParser
from urllib.parse import urlparse
import csv, json, re, sys

class Doc(HTMLParser):
    def __init__(self):
        super().__init__(); self.title=[]; self.in_title=False; self.text=[]; self.meta=[]; self.links=[]; self.jsonld=[]; self.in_jsonld=False; self.buf=[]
    def handle_starttag(self, tag, attrs):
        a={k.lower():(v or '') for k,v in attrs}; tag=tag.lower()
        if tag=='title': self.in_title=True
        if tag=='meta': self.meta.append(a)
        if tag=='link': self.links.append(a)
        if tag=='a' and a.get('href'): self.links.append(a)
        if tag=='script' and a.get('type','').lower()=='application/ld+json': self.in_jsonld=True; self.buf=[]
    def handle_endtag(self, tag):
        if tag.lower()=='title': self.in_title=False
        if tag.lower()=='script' and self.in_jsonld:
            self.in_jsonld=False
            try: self.jsonld.append(json.loads(''.join(self.buf)))
            except Exception: pass
    def handle_data(self, data):
        if self.in_title: self.title.append(data)
        if self.in_jsonld: self.buf.append(data)
        else: self.text.append(data)

def norm(v): return re.sub(r'\s+', ' ', v).strip().lower()
def objects(v):
    if isinstance(v, dict):
        yield v
        for x in v.values(): yield from objects(x)
    elif isinstance(v, list):
        for x in v: yield from objects(x)

base=Path('tasks/programmatic-seo'); out=base/'output'; errors=[]; titles=[]; descriptions=[]
rows=list(csv.DictReader((base/'integrations.csv').open()))
index=out/'index.html'
if not index.is_file(): errors.append('missing index.html')
else:
    d=Doc(); d.feed(index.read_text(errors='ignore'))
    hrefs=' '.join(a.get('href','') for a in d.links)
    for row in rows:
        if f'pages/{row["slug"]}.html' not in hrefs: errors.append(f'index misses {row["slug"]}')
for row in rows:
    path=out/'pages'/f'{row["slug"]}.html'
    if not path.is_file(): errors.append(f'missing {path}'); continue
    raw=path.read_text(errors='ignore'); d=Doc(); d.feed(raw); visible=norm(' '.join(d.text))
    if len(visible)<450: errors.append(f'{path} is too thin')
    for value in [row['name'], row['setup_minutes'], row['best_for']]:
        if norm(value) not in visible: errors.append(f'{path} misses source value: {value}')
    expected=f'https://relayops.example/integrations/{row["slug"]}'
    canon=[a.get('href','') for a in d.links if 'canonical' in a.get('rel','').lower().split()]
    valid_canon={expected,expected+'/',expected+'.html'}
    if len(canon)!=1 or canon[0] not in valid_canon: errors.append(f'{path} canonical must resolve to the integration slug under the production origin')
    title=norm(' '.join(d.title)); desc=[norm(a.get('content','')) for a in d.meta if a.get('name','').lower()=='description']
    if not title or len(desc)!=1 or len(desc[0])<40: errors.append(f'{path} lacks a complete title or meta description')
    else: titles.append(title); descriptions.append(desc[0])
    faq=[]
    for blob in d.jsonld:
        for obj in objects(blob):
            if obj.get('@type')=='FAQPage': faq.append(obj)
    good=False
    for obj in faq:
        qs=obj.get('mainEntity',[])
        good = good or (len(qs)>=2 and all(q.get('@type')=='Question' and norm(q.get('name','')) and isinstance(q.get('acceptedAnswer'),dict) and q['acceptedAnswer'].get('@type')=='Answer' and norm(q['acceptedAnswer'].get('text','')) for q in qs))
    if not good: errors.append(f'{path} lacks substantive FAQPage questions and answers')
    hrefs=[urlparse(a.get('href','')).path.rsplit('/',1)[-1].removesuffix('.html') for a in d.links]
    if len({s for s in hrefs if s in {r['slug'] for r in rows} and s!=row['slug']})<2: errors.append(f'{path} lacks two useful integration links')
    for pattern in [r'\btrusted by\s+\d', r'\b\d(?:\.\d)?\s*/\s*5\s+stars?', r'\baward[- ]winning\b']:
        if re.search(pattern, visible): errors.append(f'{path} contains unsupported proof')
if len(set(titles))!=len(rows): errors.append('page titles are not unique')
if len(set(descriptions))!=len(rows): errors.append('meta descriptions are not unique')
for e in errors: print('FAIL',e)
sys.exit(0 if not errors else 1)
PYEOF
