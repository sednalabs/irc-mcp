"""Executable artifact/schema/fixture checks, NOT Rust runtime tests."""
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path
import pytest
from jsonschema import Draft202012Validator
ROOT = Path(__file__).resolve().parents[1]
TOOLS = json.loads((ROOT/'schema/tools.json').read_text())
BY_NAME = {t['name']: t for t in TOOLS}

@pytest.mark.parametrize('tool', TOOLS, ids=lambda t: t['name'])
def test_tool_schemas_are_valid_and_closed(tool):
    Draft202012Validator.check_schema(tool['inputSchema'])
    Draft202012Validator.check_schema(tool['outputSchema'])
    assert tool['inputSchema']['additionalProperties'] is False
    assert tool['inputSchema']['type'] == 'object'
    assert not {'host','password','command','url','raw'} & set(tool['inputSchema']['properties'])

@pytest.mark.parametrize('name,args', [
 ('irc_status', {}), ('irc_channels', {}),
 ('irc_channel_info', {'target':'pilot'}),
 ('irc_read', {'target':'pilot','limit':20}),
 ('irc_inbox', {}),
 ('irc_inbox', {'limit':20,'after':'a'*32}),
 ('irc_send', {'target':'pilot','text':'Hello from the test fixture.','request_key':'example_request_key_0001'}),
 ('irc_join', {'target':'pilot','request_key':'example_request_key_0002'}),
 ('irc_part', {'target':'pilot','request_key':'example_request_key_0003'}),
])
def test_documented_input_fixtures(name,args):
    Draft202012Validator(BY_NAME[name]['inputSchema']).validate(args)

@pytest.mark.parametrize('args', [
 {'target':'pilot','text':'/quit','request_key':'example_request_key_0001'},
 {'target':'pilot','text':' /exec anything','request_key':'example_request_key_0001'},
 {'target':'pilot','text':'hello\r\nQUIT','request_key':'example_request_key_0001'},
 {'target':'pilot','text':'hi','request_key':'short'},
 {'target':'pilot','text':'hi','request_key':'example_request_key_0001','command':'/quit'},
])
def test_unsafe_send_fixtures_fail_schema(args):
    assert list(Draft202012Validator(BY_NAME['irc_send']['inputSchema']).iter_errors(args))

@pytest.mark.parametrize('args', [
 {'target':'pilot','limit':0}, {'target':'pilot','limit':101},
 {'target':'pilot','after':None}, {'target':'pilot','limit':1.5},
 {'target':'pilot','password':'not-a-field'},
])
def test_invalid_read_fixtures(args):
    assert list(Draft202012Validator(BY_NAME['irc_read']['inputSchema']).iter_errors(args))

@pytest.mark.parametrize('tool',TOOLS,ids=lambda t:t['name'])
def test_output_envelopes(tool):
    validate=Draft202012Validator(tool['outputSchema'])
    data=json.loads((ROOT/'tests/fixtures/inbox_page.json').read_text()) if tool['name']=='irc_inbox' else {'example':True}
    validate.validate({'ok':True,'data':data})
    validate.validate({'ok':False,'error':{'code':'example','message':'Example error.'}})
    assert list(validate.iter_errors({'ok':True}))

@pytest.mark.parametrize('tool', TOOLS, ids=lambda t: t['name'])
def test_chatgpt_tool_metadata(tool):
    assert tool['title'].strip()
    assert tool['annotations']['title'] == tool['title']
    a=tool['annotations']
    assert isinstance(a['readOnlyHint'], bool)
    assert isinstance(a['destructiveHint'], bool)
    assert a['openWorldHint'] is False
    assert a['readOnlyHint'] is (tool['name'] not in {'irc_send','irc_join','irc_part'})
    assert a['destructiveHint'] is (tool['name'] in {'irc_send','irc_part'})
    scopes=['irc.read','irc.write'] if tool['name'] in {'irc_send','irc_join','irc_part'} else ['irc.read']
    schemes=[{'type':'oauth2','scopes':scopes}]
    assert tool['securitySchemes'] == schemes
    assert tool['_meta']['securitySchemes'] == schemes
    assert tool['_meta']['ui']['visibility'] == ['model']

def test_tool_inventory_is_small():
    assert len(TOOLS)==8 and len(BY_NAME)==8
    assert all(not re.search(r'raw|exec|shell|script', t['name']) for t in TOOLS)
    assert not BY_NAME['irc_send']['annotations']['idempotentHint']

def test_all_toml_parses():
    for p in ROOT.rglob('*.toml'):
        tomllib.loads(p.read_text())

def test_pinned_dependencies_and_explicit_root_patch():
    cargo=tomllib.loads((ROOT/'Cargo.toml').read_text())
    p=json.loads((ROOT/'PROVENANCE.json').read_text())
    assert cargo['dependencies']['mcp-toolkit']['rev']==p['toolkit']['commit']
    assert cargo['patch']['crates-io']['rmcp']['rev']==p['rmcp']['commit']
    assert cargo['patch']['crates-io']['rmcp-macros']['rev']==p['rmcp']['commit']
    assert cargo['dependencies']['mcp-toolkit-resource-state']['path']=='upstream/mcp-toolkit-resource-state'

def test_example_configuration_is_private_and_bounded():
    c=tomllib.loads((ROOT/'config.example.toml').read_text())
    assert c['bind'].startswith('127.0.0.1:')
    assert c['public_base_url'].startswith('https://')
    assert c['relay_socket'].startswith('/')
    assert 0<len(c['conversations'])<=32
    assert len({x['key'] for x in c['conversations']})==len(c['conversations'])

def test_helper_has_no_dependencies():
    c=tomllib.loads((ROOT/'upstream/mcp-toolkit-resource-state/Cargo.toml').read_text())
    assert not c['dependencies']

def test_fixture_has_explicit_optional_irc_metadata():
    v=json.loads((ROOT/'tests/fixtures/line.json').read_text())
    assert v['id']==12
    assert 'irc_privmsg' in v['tags']
    assert any(t.startswith('irc_tag_msgid_') for t in v['tags'])

def test_patch_applies_to_captured_manifest_prefix_and_matches_staged_source(tmp_path):
    before='[workspace]\nresolver = "2"\nmembers = [\n  "crates/mcp-toolkit",\n  "crates/mcp-toolkit-core",\n  "crates/mcp-toolkit-auth",\n]\n'
    (tmp_path/'Cargo.toml').write_text(before)
    subprocess.run(['git','init','-q',str(tmp_path)],check=True)
    patch=ROOT/'upstream/toolkit-resource-state.patch'
    subprocess.run(['git','apply','--check',str(patch)],cwd=tmp_path,check=True,capture_output=True)
    subprocess.run(['git','apply',str(patch)],cwd=tmp_path,check=True,capture_output=True)
    staged=ROOT/'upstream/mcp-toolkit-resource-state'
    for p in staged.rglob('*'):
        if p.is_file():
            assert (tmp_path/'crates/mcp-toolkit-resource-state'/p.relative_to(staged)).read_bytes()==p.read_bytes()

def test_patch_is_deterministic():
    path=ROOT/'upstream/toolkit-resource-state.patch'; before=path.read_bytes()
    subprocess.run([sys.executable,str(ROOT/'scripts/generate-toolkit-patch.py')],check=True)
    assert path.read_bytes()==before

def test_example_contains_paths_not_secrets():
    c=tomllib.loads((ROOT/'config.example.toml').read_text())
    assert 'password' not in c and 'token' not in c and 'client_secret' not in c
    assert c['relay_password_file'].endswith('.secret')
    assert all('password' not in v for v in c['conversations'])


def test_no_unimplemented_rust_placeholders():
    for p in ROOT.rglob('*.rs'):
        assert not re.search(r'\b(?:todo|unimplemented)!\s*\(',p.read_text()),p

@pytest.mark.parametrize('args', [
 {'limit':0},{'limit':101},{'limit':1.5},{'after':None},{'after':'short'},
 {'target':'pilot'},{'priority':'steer'},{'watched_channels':['private']},{'ack':True},
])
def test_invalid_inbox_arguments(args):
    assert list(Draft202012Validator(BY_NAME['irc_inbox']['inputSchema']).iter_errors(args))

def test_attention_defaults_are_receiver_controlled():
    c=tomllib.loads((ROOT/'config.example.toml').read_text())
    a=c['attention']
    assert a['direct_messages'] and a['mentions']
    assert not a['watched_channels'] and not a['mention_aliases']
    assert 0<a['max_entries']<=1024 and a['max_bytes']<=4*1024*1024
    assert 0<a['max_age_seconds']<=86400
    assert 'attention' not in BY_NAME['irc_inbox']['inputSchema']['properties']


def test_typed_inbox_page_fixture_and_tool_contract_agree():
    schema=json.loads((ROOT/'schema/inbox-page.json').read_text())
    data=json.loads((ROOT/'tests/fixtures/inbox_page.json').read_text())
    Draft202012Validator.check_schema(schema)
    Draft202012Validator(schema).validate(data)
    assert {k:v for k,v in schema.items() if k!='$schema'}==BY_NAME['irc_inbox']['outputSchema']['properties']['data']
    assert data['items'][0]['original_text_bytes']==len(data['items'][0]['message']['text'].encode())

@pytest.mark.parametrize('field,value', [('consumption','read'),('earlier_history_complete',True),('model_wake','performed')])
def test_inbox_contract_rejects_unproved_delivery_claims(field,value):
    data=json.loads((ROOT/'tests/fixtures/inbox_page.json').read_text())
    data[field]=value
    assert list(Draft202012Validator(BY_NAME['irc_inbox']['outputSchema']['properties']['data']).iter_errors(data))
