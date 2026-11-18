import re

LABELS = re.compile(r'<div class="tabbed-labels">(.*?)</div>', re.S)
LABEL = re.compile(r"<label[^>]*>(.*?)</label>", re.S)
BLOCK = '<div class="tabbed-block">'


def preprocess(html, output):
    out, pos = [], 0
    for labels in LABELS.finditer(html):
        out.append(html[pos : labels.end()])
        pos = labels.end()
        for label in LABEL.findall(labels.group(1)):
            block = html.index(BLOCK, pos) + len(BLOCK)
            out.append(html[pos:block])
            out.append(f"<p><strong>{label}</strong></p>")
            pos = block
    out.append(html[pos:])
    return "".join(out)
