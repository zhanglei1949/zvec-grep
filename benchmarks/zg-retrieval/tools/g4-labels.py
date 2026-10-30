#!/usr/bin/env python3
"""Freeze source-reviewed G4 labels without invoking zg or reading its graph.

Python spans use CPython AST. Rust spans use an independent lexical brace scan;
only the explicitly reviewed free-function call pairs below are labelled. The
oracle is deliberately partial: unlabelled returned relations remain unknown.
"""
import ast
import hashlib
import json
import pathlib
import re
import sys

BASE = pathlib.Path(__file__).resolve().parents[1]


def digest(data):
    return hashlib.sha256(data).hexdigest()


def mask_rust(text):
    out = list(text)
    i = 0
    while i < len(text):
        end = None
        if text.startswith('//', i):
            end = text.find('\n', i)
            if end < 0:
                end = len(text)
        elif text.startswith('/*', i):
            depth, j = 1, i + 2
            while j < len(text) and depth:
                if text.startswith('/*', j):
                    depth += 1
                    j += 2
                elif text.startswith('*/', j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            end = j
        else:
            raw = re.match(r'(?:br|r)(#{0,16})"', text[i:])
            if raw:
                close = '"' + raw.group(1)
                j = text.find(close, i + raw.end())
                if j < 0:
                    raise ValueError('unterminated raw string')
                end = j + len(close)
            elif text[i] == '"':
                j = i + 1
                while j < len(text):
                    if text[j] == '\\':
                        j += 2
                    elif text[j] == '"':
                        j += 1
                        break
                    else:
                        j += 1
                end = j
            elif text[i] == "'":
                char = re.match(r"'(?:\\(?:u\{[0-9a-fA-F]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'", text[i:])
                if char:
                    end = i + char.end()
        if end is None:
            i += 1
        else:
            for k in range(i, end):
                if text[k] != '\n':
                    out[k] = ' '
            i = end
    return ''.join(out)


class Source:
    def __init__(self, root, language):
        self.root, self.language, self.cache = root, language, {}

    def load(self, path):
        if path not in self.cache:
            data = (self.root / path).read_bytes()
            self.cache[path] = (data, data.decode('utf-8'))
        return self.cache[path]

    def evidence(self, path, start, end):
        data, _ = self.load(path)
        return dict(path=path, sha256=digest(data), start=start, end=end,
                    quote=data[start:end].decode('utf-8'))

    def node(self, path, name):
        data, text = self.load(path)
        if self.language == 'python':
            tree = ast.parse(text)
            matches = [n for n in ast.walk(tree)
                       if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)) and n.name == name]
            assert len(matches) == 1, (path, name, len(matches))
            n = matches[0]
            lines = data.splitlines(keepends=True)
            start = sum(map(len, lines[:n.lineno - 1])) + n.col_offset
            end = sum(map(len, lines[:n.end_lineno - 1])) + n.end_col_offset
        else:
            masked = mask_rust(text)
            matches = list(re.finditer(r'(?m)^[ \t]*(?:pub(?:\([^\n]*?\))?\s+)?(?:async\s+)?fn\s+' + re.escape(name) + r'\b', masked))
            assert len(matches) == 1, (path, name, len(matches))
            m = matches[0]
            a = m.start() + len(m.group()[0:len(m.group()) - len(m.group().lstrip())])
            j = masked.index('{', m.end())
            depth, end = 1, j + 1
            while depth:
                depth += (masked[end] == '{') - (masked[end] == '}')
                end += 1
            start, end = len(text[:a].encode()), len(text[:end].encode())
        return dict(path=path, name=name, span=dict(start=start, end=end),
                    sha256=digest(data))

    def edge(self, source, target):
        data, text = self.load(source['path'])
        start, end = source['span']['start'], source['span']['end']
        body = data[start:end].decode()
        masked = mask_rust(body) if self.language == 'rust' else body
        edges = []
        if self.language == 'python':
            n = ast.parse(text)
            lines = data.splitlines(keepends=True)
            for call in ast.walk(n):
                if isinstance(call, ast.Call) and isinstance(call.func, ast.Name) and call.func.id == target['name']:
                    a = sum(map(len, lines[:call.func.lineno - 1])) + call.func.col_offset
                    b = a + len(target['name'].encode())
                    if start <= a < end:
                        edges.append(dict(source=source, target=target, kind='calls',
                                          evidence=self.evidence(source['path'], a, b)))
        else:
            for m in re.finditer(r'(?<![\w.])' + re.escape(target['name']) + r'\s*\(', masked):
                # A declaration is not a call; all other matches are reviewed code.
                if re.search(r'\bfn\s*$', masked[max(0, m.start()-5):m.start()]):
                    continue
                a = start + len(body[:m.start()].encode())
                edges.append(dict(source=source, target=target, kind='calls',
                                  evidence=self.evidence(source['path'], a, a + len(target['name'].encode()))))
        assert edges, (source['name'], target['name'])
        return edges


def locator(node):
    return dict(by='name', name=node['name'], source_id=node['path'])


def traversal(**kwargs):
    return dict(relations=['calls'], direction='outgoing', min_hops=1,
                max_hops=2, **kwargs)


def make_tasks(root, language, repository):
    s = Source(root, language)
    if language == 'rust':
        a, b = 'crates/cli/src/escape.rs', 'crates/cli/src/pattern.rs'
        n = {name: s.node(path, name) for name, path in [
            ('escape', a), ('escape_os', a), ('unescape', a), ('unescape_os', a),
            ('pattern_from_os', b), ('pattern_from_bytes', b),
            ('patterns_from_path', b), ('patterns_from_stdin', b), ('patterns_from_reader', b)]}
        pairs = [('escape_os','escape'),('unescape_os','unescape'),
                 ('pattern_from_os','escape_os'),('pattern_from_bytes','escape'),
                 ('patterns_from_path','patterns_from_reader'),
                 ('patterns_from_stdin','patterns_from_reader'),
                 ('patterns_from_reader','pattern_from_bytes')]
        locate_names = ['escape_os','unescape_os']
        incoming = ['escape','unescape','pattern_from_bytes','patterns_from_reader']
        outgoing = ['escape_os','unescape_os','patterns_from_path','pattern_from_bytes']
        expands = ['pattern_from_os','patterns_from_path']
        path_pair = ('pattern_from_os','escape')
        absent_pair = ('escape','patterns_from_reader')
        imports = [(b, ['escape','escape_os']), ('crates/cli/src/lib.rs', ['escape','unescape'])]
        refs = ['pattern_from_os','patterns_from_reader','escape','unescape_os']
        ambiguous = 'new'
    else:
        a, b, c = 'src/requests/utils.py', 'src/requests/cookies.py', 'src/requests/sessions.py'
        names = {a: ['requote_uri','unquote_unreserved','check_header_validity','_validate_header_part',
                     'to_key_val_list','get_encoding_from_headers','_parse_content_type_header','super_len'],
                 b: ['create_cookie','morsel_to_cookie','cookiejar_from_dict','merge_cookies'],
                 c: ['merge_setting','merge_hooks']}
        n = {name: s.node(path, name) for path, names_ in names.items() for name in names_}
        pairs = [('requote_uri','unquote_unreserved'),('check_header_validity','_validate_header_part'),
                 ('morsel_to_cookie','create_cookie'),('cookiejar_from_dict','create_cookie'),
                 ('merge_cookies','cookiejar_from_dict'),('merge_hooks','merge_setting'),
                 ('merge_setting','to_key_val_list'),('get_encoding_from_headers','_parse_content_type_header')]
        locate_names = ['requote_uri','check_header_validity']
        incoming = ['unquote_unreserved','_validate_header_part','create_cookie','merge_setting']
        outgoing = ['requote_uri','check_header_validity','merge_hooks','morsel_to_cookie']
        expands = ['merge_hooks','merge_cookies']
        path_pair = ('merge_hooks','to_key_val_list')
        absent_pair = ('unquote_unreserved','merge_hooks')
        imports = [(c,['to_key_val_list']), (c,['merge_cookies'])]
        refs = ['check_header_validity','super_len','get_encoding_from_headers','merge_setting']
        ambiguous = '__init__'
    all_edges = [edge for x,y in pairs for edge in s.edge(n[x], n[y])]
    tasks = []

    def add(category, operation, truth_nodes=(), edges=(), **expect):
        task_id = ('ripgrep' if language == 'rust' else 'requests') + ':G4-' + f'{len(tasks)+1:02}'
        evidence = [s.evidence(x['path'],x['span']['start'],x['span']['end']) for x in truth_nodes]
        evidence += [x['evidence'] for x in edges]
        task = dict(task_id=task_id, repository=repository, language=language, category=category,
                    explore_request=dict(domain='code', operation=operation),
                    truth=dict(nodes=list(truth_nodes), edges=list(edges), required_evidence=evidence,
                               exhaustive=False, unknown_relations='unlabelled outputs are unknown, not false'),
                    contract={'locations':['unique'], **expect},
                    annotation=dict(status='source_reviewed', basis='Direct source inspection and independent CPython AST / lexical Rust spans; no zg extractor or graph outputs.',
                                    human_reviewed=False))
        tasks.append(task)
        return task

    def edges_from(name):
        return [x for x in all_edges if x['source']['name']==name]

    for name in locate_names:
        add('locate',dict(operation='locate',target=locator(n[name])),[n[name]])
    add('locate',dict(operation='locate',target=dict(by='name',name=ambiguous)),locations=['ambiguous'])
    absent='__g4_absent_symbol_20261029__'
    assert not any(absent.encode() in p.read_bytes() for p in root.rglob('*.rs' if language=='rust' else '*.py'))
    add('locate',dict(operation='locate',target=dict(by='name',name=absent)),locations=['not_found'])
    for name in incoming:
        edges=[x for x in all_edges if x['target']['name']==name]
        add('callers',dict(operation='neighbors',anchor=locator(n[name]),relations=['calls'],direction='incoming'),
            [n[name]] + [x['source'] for x in edges],edges)
    for name in outgoing:
        edges=edges_from(name)
        add('callees',dict(operation='neighbors',anchor=locator(n[name]),relations=['calls'],direction='outgoing'),
            [n[name]] + [x['target'] for x in edges],edges)
    for path in [a,b]:
        nodes=[x for x in n.values() if x['path']==path]
        add('contains',dict(operation='neighbors',anchor=dict(by='position',source_id=path,byte_offset=0,kind='file'),relations=['contains'],direction='outgoing'),nodes)
    for path,names_ in imports:
        add('imports',dict(operation='neighbors',anchor=dict(by='position',source_id=path,byte_offset=0,kind='file'),relations=['imports'],direction='outgoing'),[n[x] for x in names_])
    for name in expands:
        first=edges_from(name); second=[y for x in first for y in edges_from(x['target']['name'])];edges=first+second
        add('expand',dict(operation='expand',anchor=locator(n[name]),traversal=traversal()),
            [x['target'] for x in edges],edges)
    x,y=path_pair
    chain=[n[x]]
    current=x
    while current!=y:
        edge=edges_from(current)[0];current=edge['target']['name'];chain.append(n[current])
    add('path',dict(operation='shortest_path',start=locator(n[x]),target=locator(n[y]),traversal=traversal()),chain,
        path_names=[z['name'] for z in chain],path_hops=len(chain)-1,locations=['unique','unique'])
    x,y=absent_pair
    add('path',dict(operation='shortest_path',start=locator(n[x]),target=locator(n[y]),traversal=traversal()),
        [n[x],n[y]],path_hops=None,locations=['unique','unique'])
    for name in refs:
        edges=edges_from(name)
        task=add('references',dict(operation='references',selector=dict(by='owner',owner=locator(n[name]))),
                 [n[name]],edges)
        dynamic = ({'escape':'bytes.escape_bytes'} if language=='rust' else
                   {'super_len':'o.fileno','get_encoding_from_headers':'headers.get','merge_setting':'dict_class'})
        if name in dynamic:
            node=n[name]; data,_=s.load(node['path']); token=dynamic[name].encode()
            start=data.index(token,node['span']['start'],node['span']['end'])
            site=s.evidence(node['path'],start,start+len(token))
            task['truth']['reference_sites']=[dict(evidence=site,target_known=False)]
            task['contract']['must_not_resolve']=[site]
    assert len(tasks)==24
    chosen=[0,1,4,5,6,7,8,9,10,11,16,17]
    scopes={0,1,8,9,10,11,16,17}
    for i in chosen:
        task=tasks[i];op=task['explore_request']['operation'];anchor=op.get('anchor',op.get('target'))
        name=anchor['name'];node=n[name]
        query=f"How does {name} in {node['path']} work, and which functions supply its behavior?"
        task['retrieval']=dict(query=query,query_sha256=digest(query.encode()),
            augment=dict(mode='augment',seeds=5,expansion=traversal()),
            target_files=sorted({x['path'] for x in task['truth']['nodes']}),
            required_evidence=task['truth']['required_evidence'])
        if i in scopes:
            # A function and its nested declarations stay inside the same full
            # function span; this scope has an independently complete boundary.
            scope=dict(anchor=locator(node),traversal=dict(relations=['contains'],direction='outgoing',min_hops=1,max_hops=1,include_start=True),region='full')
            task['retrieval']['constrain']=dict(mode='constrain',scope=scope)
            task['retrieval']['allowed_regions']=[s.evidence(node['path'],node['span']['start'],node['span']['end'])]
            task['retrieval']['scope_required_evidence']=task['retrieval']['allowed_regions']
    return tasks


def main():
    paths=json.loads(pathlib.Path(sys.argv[1]).read_text())
    reports=pathlib.Path(paths['reports'])
    repos=json.loads((reports/'repositories.json').read_text())
    tasks=[]
    for repo in repos:
        language='rust' if 'ripgrep' in repo['repository'] else 'python'
        repo['language']=language
        source=pathlib.Path(repo.pop('root'))
        repo.pop('remote_default_evidence',None)
        frozen=json.loads((reports/repo['repository'].replace('/','__')/'selection.json').read_text())
        repo['selection']=frozen
        tasks.extend(make_tasks(source,language,repo['repository']))
    inputs=dict(schema_version=1,id='g4-code-source-v1',repositories=repos,
                annotation_tool_sha256=digest(pathlib.Path(__file__).read_bytes()),tasks=tasks)
    output=BASE/'data/g4-code-v1.json'
    assert not output.exists(),'frozen labels already exist; create a new version instead'
    output.write_text(json.dumps(inputs,ensure_ascii=False,indent=2)+'\n')
    print(json.dumps(dict(tasks=len(tasks),retrieval=sum('retrieval' in x for x in tasks),
                          constrain=sum('constrain' in x.get('retrieval',{}) for x in tasks),sha256=digest(output.read_bytes()))))


if __name__=='__main__':
    main()
