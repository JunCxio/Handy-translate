"""使用本机模拟翻译接口验证完整流程，不调用云端服务。"""
import json
import base64
import os
import queue
import re
import shutil
import subprocess
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


class MockProvider(BaseHTTPRequestHandler):
    records = []
    payloads = []
    received = threading.Event()
    invalid = False
    lock = threading.Lock()
    active = 0
    maximum = 0
    delay = 0.2
    fail_first = False
    fail_at = 0
    anthropic_auth = 'api_key'
    anthropic_reason = 'end_turn'
    image_records = []
    image_invalid = False
    image_empty = False

    def log_message(self, *_):
        pass

    def send_json(self, response):
        payload = json.dumps(response).encode()
        try:
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            pass

    def do_GET(self):
        if self.path == '/v1/models':
            if self.headers['anthropic-version']:
                self.check_anthropic_auth()
            else:
                assert self.headers['Authorization'] == 'Bearer session-only-secret'
            self.send_json({'data':[{'id':'mock'},{'id':'other'},{'id':'mock'}]})
        else:
            self.send_error(404)

    def do_POST(self):
        value = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        content = value.get('input') if self.path == '/v1/responses' else value.get('messages',[{}])[-1].get('content')
        if isinstance(content,list):
            if self.path == '/v1/messages':
                self.check_anthropic_auth()
                encoded = content[0]['source']['data']
            elif self.path == '/v1/responses':
                assert self.headers['Authorization'] == 'Bearer session-only-secret'
                assert value['store'] is False and value['stream'] is False
                encoded = content[0]['content'][1]['image_url'].split(',',1)[1]
            else:
                assert self.headers['Authorization'] == 'Bearer session-only-secret'
                encoded = content[1]['image_url']['url'].split(',',1)[1]
            assert base64.b64decode(encoded).startswith(b'\x89PNG\r\n\x1a\n')
            type(self).image_records.append(self.path)
            type(self).received.set()
            time.sleep(self.delay)
            text = json.dumps({'regions':[] if self.image_empty else [{'x':99999 if self.image_invalid else 10,'y':10,'width':160,'height':50,'original':'Help'}]})
            response = {'status':'completed','output':[{'type':'message','content':[{'type':'output_text','text':text}]}]} if self.path == '/v1/responses' else {'stop_reason':'end_turn','content':[{'type':'text','text':text}]} if self.path == '/v1/messages' else {'choices':[{'finish_reason':'stop','message':{'content':text}}]}
            self.send_json(response)
            return
        if self.path in ('/v1/chat/completions', '/v1/responses', '/v1/messages'):
            if self.path == '/v1/messages':
                self.check_anthropic_auth()
                assert value['max_tokens'] == 8192 and 'untrusted data' in value['system']
                items = json.loads(value['messages'][0]['content'])
            else:
                assert self.headers['Authorization'] == 'Bearer session-only-secret'
                items = json.loads(value['input'] if self.path == '/v1/responses' else value['messages'][1]['content'])
            cls = type(self)
            with cls.lock:
                cls.records.append([item['text'] for item in items])
                cls.payloads.append(items)
                first = len(cls.records) == 1
                fail_this = cls.fail_at == len(cls.records)
                cls.active += 1
                cls.maximum = max(cls.maximum, cls.active)
            cls.received.set()
            time.sleep(cls.delay * (0.4 if first or fail_this else 1))
            with cls.lock:
                cls.active -= 1
            result = [{'id': item['id'], 'text': '译文：' + item['text']} for item in items]
            if self.invalid or (self.fail_first and first) or fail_this:
                result[0]['id'] = 99999
            text = json.dumps({'translations': result})
            if self.path == '/v1/responses':
                assert value['store'] is False and value['stream'] is False
                assert 'untrusted data' in value['instructions']
                response = {'status': 'completed', 'output': [{'type': 'message', 'content': [{'type': 'output_text', 'text': text}]}]}
            elif self.path == '/v1/messages':
                response = {'stop_reason':self.anthropic_reason,'content':[{'type':'thinking','thinking':'private'},{'type':'text','text':text}]}
            else:
                response = {'choices': [{'message': {'content': text}}]}
        elif self.path == '/v2/translate':
            assert self.headers['Authorization'] == 'DeepL-Auth-Key session-only-secret'
            response = {'translations': [{'text': '译文：' + text} for text in value['text']]}
        elif self.path == '/translate':
            response = {'translatedText': ['译文：' + text for text in value['q']]}
        else:
            self.send_error(404)
            return
        self.send_json(response)

    def check_anthropic_auth(self):
        assert self.headers['anthropic-version'] == '2023-06-01'
        if self.anthropic_auth == 'api_key':
            assert self.headers['x-api-key'] == 'session-only-secret' and not self.headers['Authorization']
        else:
            assert self.headers['Authorization'] == 'Bearer session-only-secret' and not self.headers['x-api-key']


class AppTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.exe = os.environ.get('HANDY_TRANSLATE_EXE')
        if not cls.exe:
            raise unittest.SkipTest('请设置 HANDY_TRANSLATE_EXE 为已构建的程序路径')
        cls.provider = ThreadingHTTPServer(('127.0.0.1', 0), MockProvider)
        cls.thread = threading.Thread(target=cls.provider.serve_forever, daemon=True)
        cls.thread.start()

    @classmethod
    def tearDownClass(cls):
        cls.provider.shutdown()
        cls.provider.server_close()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='handy-translate-', dir=os.environ.get('HANDY_TRANSLATE_TEST_DIR'))
        self.root = Path(self.temp.name)
        self.game = self.root / 'sample-game'
        (self.game / 'js/plugins').mkdir(parents=True)
        (self.game / 'data').mkdir()
        (self.game / 'js/rmmz_core.js').write_text('// integration fixture', 'utf-8')
        (self.game / 'js/plugins.js').write_text('var $plugins = [];\n', 'utf-8')
        items = [None] + [{'name': f'Potion {i}', 'description': f'Hello \\V[{i}]'} for i in range(13)]
        (self.game / 'data/Items.json').write_text(json.dumps(items), 'utf-8')
        self.original_data = (self.game / 'data/Items.json').read_bytes()
        MockProvider.records = []
        MockProvider.payloads = []
        MockProvider.image_records = []
        MockProvider.image_invalid = False
        MockProvider.image_empty = False
        MockProvider.received.clear()
        MockProvider.invalid = False
        MockProvider.fail_first = False
        MockProvider.fail_at = 0
        MockProvider.maximum = 0
        MockProvider.delay = 0.2
        MockProvider.anthropic_auth = 'api_key'
        MockProvider.anthropic_reason = 'end_turn'
        self.extra_env = {}
        self.start_app()

    def tearDown(self):
        self.stop_app()
        self.temp.cleanup()

    def start_app(self):
        self.process = subprocess.Popen([self.exe, '--headless', '--data-dir', str(self.root / 'state')], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding='utf-8', env={**os.environ, **self.extra_env})
        lines = queue.Queue()
        threading.Thread(target=lambda: [lines.put(line.strip()) for line in self.process.stdout], daemon=True).start()
        deadline = time.monotonic() + 25
        while time.monotonic() < deadline:
            line = lines.get(timeout=25)
            if line.startswith('http://127.0.0.1:'):
                self.base, self.token = line.split('/#')
                break
        else:
            self.fail('程序未启动')

    def stop_app(self):
        if getattr(self, 'process', None) and self.process.poll() is None:
            self.process.terminate()
            self.process.wait(timeout=10)
        if getattr(self, 'process', None) and self.process.stdout:
            self.process.stdout.close()

    def api(self, route, data=None, expected=200, headers=None):
        request = urllib.request.Request(self.base + '/api/' + route, data=None if data is None else json.dumps(data).encode(), headers={'Content-Type': 'application/json', 'X-Session-Token': self.token, **(headers or {})})
        try:
            with urllib.request.urlopen(request, timeout=15) as response:
                self.assertEqual(response.status, expected)
                return json.load(response)
        except urllib.error.HTTPError as error:
            self.assertEqual(error.code, expected, error.read().decode())
            return None

    def scan(self, target='zh-CN'):
        self.api('scan', {'path': str(self.game), 'source_language': 'en', 'target_language': target})

    def translate(self, provider='openai', concurrency=4, batch_size=48):
        path = {'openai': '/v1', 'responses': '/v1', 'deepl': '/v2', 'libretranslate': ''}[provider]
        self.api('translate', {'provider': provider, 'base_url': f'http://127.0.0.1:{self.provider.server_port}{path}', 'model': 'mock', 'api_key': 'session-only-secret', 'concurrency':concurrency, 'batch_size':batch_size})

    def finish(self, timeout=20):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            state = self.api('state')
            if not state['busy']:
                return state
            time.sleep(0.06)
        self.fail('任务超时')

    def test_openai_pause_resume_restart_and_installation(self):
        self.api('state', expected=401, headers={'X-Session-Token': 'wrong'})
        self.api('state', expected=403, headers={'Origin': 'https://untrusted.example'})
        self.scan()
        self.translate(concurrency=2, batch_size=4)
        self.assertTrue(MockProvider.received.wait(5))
        self.api('stop', {})
        state = self.finish()
        self.assertGreater(state['catalog']['translated'], 0)
        self.assertLess(state['catalog']['translated'], 26)
        self.assertFalse(state['task_succeeded'])
        first_count = state['catalog']['translated']
        self.stop_app()
        self.start_app()
        self.assertEqual(self.api('state')['catalog']['translated'], first_count)
        self.assertFalse((self.game / 'data/GameTranslatorTranslations.json').exists())
        self.scan()
        self.assertEqual(self.api('state')['catalog']['translated'], first_count)
        self.translate()
        state = self.finish()
        self.assertEqual(state['catalog']['translated'], 26)
        self.assertTrue(state['task_succeeded'])
        sent = [item for batch in MockProvider.records for item in batch]
        self.assertEqual(len(sent), len(set(sent)))
        for file in (self.root / 'state').glob('*.json'):
            self.assertNotIn('session-only-secret', file.read_text('utf-8'))
        self.api('install', {})
        self.assertTrue((self.game / 'js/plugins/GameTranslator.js').is_file())
        self.assertEqual((self.game / 'data/Items.json').read_bytes(), self.original_data)
        plugins = self.game / 'js/plugins.js'
        plugins.write_text(plugins.read_text('utf-8') + '// preserved user edit\n', 'utf-8')
        self.api('uninstall', {})
        self.assertEqual(plugins.read_text('utf-8'), 'var $plugins = [];\n// preserved user edit\n')
        self.assertFalse((self.game / 'data/GameTranslatorTranslations.json').exists())
        self.scan()
        self.assertEqual(self.api('state')['catalog']['translated'], 26)
        self.scan('zh-TW')
        self.assertFalse(self.api('state')['task_succeeded'])
        self.assertEqual(self.api('state')['catalog']['translated'], 0)

    def test_other_translation_protocols(self):
        for provider, target in [('responses', 'zh-CN'), ('deepl', 'zh-TW'), ('libretranslate', 'ko')]:
            self.scan(target)
            self.translate(provider)
            self.assertEqual(self.finish()['catalog']['translated'], 26)

    def test_pagination_clamps_to_last_nonempty_page(self):
        items = [None] + [{'name': f'Potion {i}', 'description': f'Description {i}'} for i in range(26)]
        item_file = self.game / 'data/Items.json'
        item_file.write_text(json.dumps(items), 'utf-8')
        self.scan()
        page = self.api('entries?page=99')
        self.assertEqual((page['page'], len(page['entries'])), (1, 2))
        item_file.write_text(json.dumps(items[:-1]), 'utf-8')
        self.scan()
        page = self.api('entries?page=1')
        self.assertEqual((page['page'], len(page['entries'])), (0, 50))
        page = self.api('entries?page=99&query=no-match')
        self.assertEqual((page['page'], len(page['entries'])), (0, 0))

    def test_rejects_bad_model_response_and_invalid_import_atomically(self):
        self.scan()
        MockProvider.invalid = True
        self.translate()
        state = self.finish()
        self.assertEqual(state['catalog']['translated'], 0)
        self.assertGreater(state['catalog']['failed'], 0)
        bundle = self.api('export')
        bundle['translations'] = {'Potion 0': '药水', 'unknown text': '不应写入'}
        self.api('import', bundle, expected=400)
        self.assertEqual(self.api('state')['catalog']['translated'], 0)
        row = next(e for e in self.api('entries')['entries'] if '\\V[' in e['original'])
        self.api('entry', {'id': row['id'], 'translation': '丢失变量'}, expected=400)
        self.assertEqual(self.api('state')['catalog']['translated'], 0)

    def test_concurrent_batches_merge_and_drain_other_results_after_one_fails(self):
        self.scan()
        self.translate(concurrency=3, batch_size=4)
        state = self.finish()
        self.assertEqual(state['catalog']['translated'], 26)
        self.assertEqual(MockProvider.maximum, 3)
        sent = [text for batch in MockProvider.records for text in batch]
        self.assertEqual(len(sent), 26)
        self.assertEqual(len(set(sent)), 26)
        self.scan('zh-TW')
        MockProvider.records = []
        MockProvider.fail_at = 2
        self.translate(concurrency=3, batch_size=4)
        state = self.finish()
        self.assertEqual(state['catalog']['translated'], 12)
        self.assertEqual(state['catalog']['failed'], 4)
        self.assertFalse(state['task_succeeded'])
        self.assertEqual(len(MockProvider.records), 4)

    def test_text_first_batch_failure_does_not_dispatch_other_requests(self):
        self.scan();MockProvider.fail_first=True
        self.translate(concurrency=4,batch_size=4)
        state=self.finish()
        self.assertEqual(state['catalog']['translated'],0)
        self.assertEqual(state['catalog']['failed'],4)
        self.assertEqual(state['progress']['translation_requests'],1)
        self.assertEqual(len(MockProvider.records),1)
        self.assertFalse(state['task_succeeded'])

    def test_force_close_without_installation_keeps_saved_translations(self):
        items = [None] + [{'name':f'Line {i}'} for i in range(120)]
        (self.game/'data/Items.json').write_text(json.dumps(items), 'utf-8')
        self.scan()
        self.translate(concurrency=3, batch_size=4)
        deadline = time.monotonic()+10
        while self.api('state')['catalog']['translated'] == 0:
            self.assertLess(time.monotonic(), deadline)
            time.sleep(0.01)
        self.stop_app()
        self.assertFalse((self.game/'data/GameTranslatorTranslations.json').exists())
        self.start_app()
        saved = self.api('export')['translations']
        self.assertGreater(len(saved), 0)
        self.assertLess(len(saved), 120)
        self.scan()
        self.assertEqual(self.api('export')['translations'], saved)
        time.sleep(0.3)
        MockProvider.records = []
        self.translate(concurrency=3, batch_size=16)
        self.assertEqual(self.finish()['catalog']['translated'], 120)
        requested = {text for batch in MockProvider.records for text in batch}
        self.assertTrue(requested.isdisjoint(saved.keys()))

    def test_settings_models_and_connection_survive_restart(self):
        settings = {'provider':'responses','base_url':f'http://127.0.0.1:{self.provider.server_port}/v1/responses','model':'mock','model_options':['mock','other'],'api_key':'session-only-secret','remember_key':True,'source_language':'ja','target_language':'zh-CN','concurrency':3,'batch_size':32}
        self.api('settings',settings)
        file = self.root/'state/settings.json'
        self.assertNotIn('session-only-secret',file.read_text('utf-8'))
        self.assertEqual(self.api('models',settings)['models'], ['mock','other'])
        self.assertTrue(self.api('test-service',settings)['ok'])
        self.stop_app(); self.start_app()
        restored = self.api('settings')
        for key in settings:
            self.assertEqual(restored[key],settings[key])
        settings['remember_key'] = False
        self.api('settings',settings)
        self.assertEqual(self.api('settings')['api_key'],'')
        settings['concurrency'] = 99
        self.api('translate',settings,expected=400)

    def test_auto_language_cache_and_installation_status(self):
        items = [None, {'name':'こんにちは','description':'今日はいい天気ですね。一緒に町に行きましょう。買い物が終わったら家に戻って休みましょう。'}]
        (self.game/'data/Items.json').write_text(json.dumps(items),'utf-8')
        self.api('scan',{'path':str(self.game),'source_language':'auto','target_language':'zh-CN'})
        self.assertEqual(self.api('state')['catalog']['source_language'],'ja')
        self.translate()
        self.assertEqual(self.finish()['catalog']['translated'],2)
        self.api('install',{})
        status = self.api('state')['catalog']['installation']
        self.assertTrue(status['current'])
        self.assertEqual(status['translated'],2)
        self.api('scan',{'path':str(self.game),'source_language':'ja','target_language':'zh-CN'})
        self.assertEqual(self.api('state')['catalog']['translated'],2)
        entry = self.api('entries')['entries'][0]
        self.api('entry',{'id':entry['id'],'translation':'新的译文'})
        self.assertFalse(self.api('state')['catalog']['installation']['current'])
        self.api('install',{})
        self.assertTrue(self.api('state')['catalog']['installation']['current'])
        self.api('scan',{'path':str(self.game),'source_language':'auto','target_language':'fr'})
        state = self.api('state')['catalog']
        self.assertEqual(state['translated'],0)
        self.assertEqual(state['installation']['language'],'zh-CN')
        self.assertFalse(state['installation']['current'])
        self.api('scan',{'path':str(self.game),'source_language':'auto','target_language':'zh-CN'})
        self.assertEqual(self.api('state')['catalog']['translated'],2)
        self.api('scan',{'path':str(self.game),'source_language':'auto','target_language':'auto'},expected=400)

    def test_anthropic_auth_models_and_incomplete_output(self):
        settings = {'provider':'anthropic','base_url':f'http://127.0.0.1:{self.provider.server_port}/v1/messages','model':'mock','api_key':'session-only-secret'}
        self.scan()
        for auth in ['api_key','bearer']:
            settings['auth_mode'] = auth
            MockProvider.anthropic_auth = auth
            self.assertEqual(self.api('models',settings)['models'], ['mock','other'])
            self.assertTrue(self.api('test-service',settings)['ok'])
        MockProvider.anthropic_reason = 'max_tokens'
        self.api('translate',settings)
        state = self.finish()
        self.assertEqual(state['catalog']['translated'],0)
        self.assertIn('未完整完成',state['progress']['message'])
        MockProvider.anthropic_reason = 'end_turn'
        self.api('translate',settings)
        self.assertEqual(self.finish()['catalog']['translated'],26)

    def test_local_config_import_restore_and_destination_change(self):
        self.stop_app()
        codex = self.root/'codex'
        claude = self.root/'claude'
        codex.mkdir(); claude.mkdir()
        base = f'http://127.0.0.1:{self.provider.server_port}/v1'
        config = f'model = "mock"\nmodel_provider = "custom"\n[model_providers.custom]\nbase_url = "{base}"\nwire_api = "responses"\nexperimental_bearer_token = "session-only-secret"\nrequires_openai_auth = false\n'
        (codex/'config.toml').write_text(config,'utf-8')
        (claude/'settings.json').write_text(json.dumps({'model':'sonnet','env':{'ANTHROPIC_BASE_URL':base,'ANTHROPIC_API_KEY':'session-only-secret','ANTHROPIC_DEFAULT_SONNET_MODEL':'mock'}}),'utf-8')
        self.extra_env = {'CODEX_HOME':str(codex),'CLAUDE_CONFIG_DIR':str(claude)}
        self.start_app()
        for preset in ['codex','claude_code']:
            imported = self.api('local-config',{'preset':preset})
            self.assertEqual(imported['model'],'mock')
            self.assertEqual(imported['api_key'],'session-only-secret')
            settings = {**imported,'preset':preset,'source_language':'ja','target_language':'zh-CN','remember_key':False}
            self.api('settings',settings)
            self.assertNotIn('session-only-secret',(self.root/'state/settings.json').read_text('utf-8'))
            self.stop_app(); self.start_app()
            restored = self.api('settings')
            self.assertEqual(restored['preset'],preset)
            self.assertEqual(restored['api_key'],'session-only-secret')
            self.assertEqual(restored['config_warning'],'')
            self.assertTrue(self.api('test-service',restored)['ok'])
        changed = json.loads((claude/'settings.json').read_text('utf-8'))
        changed['env']['ANTHROPIC_BASE_URL'] = 'https://changed.example/v1'
        (claude/'settings.json').write_text(json.dumps(changed),'utf-8')
        self.stop_app(); self.start_app()
        restored = self.api('settings')
        self.assertEqual(restored['api_key'],'')
        self.assertIn('已改变',restored['config_warning'])
        self.api('settings',settings,expected=400)
        self.api('local-config',{'preset':'unknown'},expected=400)

    def test_speed_comparison_with_3542_entries(self):
        (self.game/'data/Items.json').write_text(json.dumps([None]+[{'name':f'Text {i}'} for i in range(3542)]),'utf-8')
        MockProvider.delay = 0.04
        results = []
        for language, concurrency, size in [('zh-CN',1,12),('zh-TW',4,48)]:
            self.scan(language)
            MockProvider.records = []; MockProvider.maximum = 0
            started = time.monotonic()
            self.translate(concurrency=concurrency,batch_size=size)
            self.assertEqual(self.finish(timeout=90)['catalog']['translated'],3542)
            elapsed = time.monotonic()-started
            self.assertEqual(len(MockProvider.records),(3542+size-1)//size)
            self.assertEqual(MockProvider.maximum,concurrency)
            results.append({'concurrency':concurrency,'batch_size':size,'requests':len(MockProvider.records),'seconds':round(elapsed,3)})
        report = {'entries':3542,'mock_request_delay_seconds':0.04,'runs':results,'speedup':round(results[0]['seconds']/results[1]['seconds'],2)}
        print('\n速度对比（模拟接口）：'+json.dumps(report,ensure_ascii=False),flush=True)


    def request_config(self, **extra):
        return {'provider':'openai', 'base_url':f'http://127.0.0.1:{self.provider.server_port}/v1',
                'model':'mock', 'api_key':'session-only-secret', 'batch_size':48, 'concurrency':4, **extra}

    def test_glossary_review_and_retranslate_preserve_old_results_on_failure(self):
        self.scan()
        row = next(e for e in self.api('entries')['entries'] if e['original'] == 'Potion 0')
        self.api('entry', {'id':row['id'],'translation':'旧译文'})
        self.api('glossary', {'terms':[{'source':'Potion 0','target':'恢复药'}]})
        self.assertEqual(self.api('review')['flagged'],1)
        self.assertIn('术语不一致',self.api('entries?filter=review')['entries'][0]['issues'][0])
        self.api('translate',self.request_config(ids=[row['id']],instructions='保留敬语'))
        state = self.finish()
        self.assertEqual(state['catalog']['translated'],1)
        self.assertEqual(state['progress']['cache_hits'],0)
        translated = next(e for e in self.api('entries')['entries'] if e['id'] == row['id'])
        self.assertEqual(translated['translation'],'译文：恢复药')
        self.assertEqual(self.api('review')['flagged'],0)
        self.assertEqual(self.api('export')['glossary'],[{'source':'Potion 0','target':'恢复药'}])
        MockProvider.invalid = True
        self.api('translate',self.request_config(ids=[row['id']]))
        state = self.finish()
        self.assertEqual(state['catalog']['translated'],1)
        self.assertEqual(state['catalog']['failed'],1)
        self.assertEqual(next(e for e in self.api('entries')['entries'] if e['id'] == row['id'])['translation'],translated['translation'])
        self.api('translate',self.request_config(ids=[]),expected=400)
        self.api('translate',self.request_config(ids=[row['id'],row['id']]),expected=400)
        self.api('translate',self.request_config(ids=['unknown']),expected=400)
        self.api('glossary',{'terms':[{'source':'x','target':'y'},{'source':'x','target':'z'}]},expected=400)
        self.stop_app(); self.start_app(); self.scan()
        self.assertEqual(self.api('glossary')['terms'],[{'source':'Potion 0','target':'恢复药'}])
        self.assertEqual(next(e for e in self.api('entries')['entries'] if e['id'] == row['id'])['translation'],translated['translation'])

    def test_exact_cache_reuses_other_games_survives_restart_and_obeys_terms(self):
        self.scan(); self.translate()
        self.assertEqual(self.finish()['catalog']['translated'],26)
        request_count = len(MockProvider.records)
        another = self.root/'another-game'
        shutil.copytree(self.game,another)
        self.stop_app(); self.start_app()
        self.api('scan',{'path':str(another),'source_language':'en','target_language':'zh-CN'})
        self.assertEqual(self.api('state')['catalog']['translated'],0)
        self.translate()
        state = self.finish()
        self.assertEqual(state['catalog']['translated'],26)
        self.assertEqual(state['progress']['cache_hits'],26)
        self.assertEqual(len(MockProvider.records),request_count)
        self.api('glossary',{'terms':[{'source':'Potion 0','target':'恢复药'}]})
        row = next(e for e in self.api('entries')['entries'] if e['original'] == 'Potion 0')
        self.api('entry',{'id':row['id'],'translation':None})
        self.translate()
        self.assertEqual(self.finish()['progress']['cache_hits'],0)
        self.assertEqual(len(MockProvider.records),request_count+1)
        self.assertEqual(next(e for e in self.api('entries')['entries'] if e['id']==row['id'])['translation'],'译文：恢复药')
        self.api('entry',{'id':row['id'],'translation':None})
        self.translate()
        self.assertEqual(self.finish()['progress']['cache_hits'],0)
        self.assertEqual(len(MockProvider.records),request_count+2)

    def image_fixture(self, copies=1):
        import struct
        import zlib
        def chunk(kind,data):
            return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data)&0xffffffff)
        png=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',200,100,8,2,0,0,0))+chunk(b'IDAT',zlib.compress((b'\0'+b'\xf0\xf0\xf0'*200)*100))+chunk(b'IEND',b'')
        (self.game/'img/pictures').mkdir(parents=True)
        for i in range(copies):
            (self.game/f'img/pictures/help{i}.png').write_bytes(png)
        (self.game/'data/Map001.json').write_text(json.dumps({'list':[{'code':231,'parameters':[1,f'help{i}']} for i in range(copies)]}),'utf-8')
        self.scan()
        self.assertFalse((self.root/'state/images').exists())
        self.assertIsNone(self.api('images/state')['project'])
        self.assertFalse((self.root/'state/images').exists())
        self.api('images/scan',{})
        return self.api('images/state')['project']['assets']

    def image_process(self, ids, action, mode='vision', provider='openai', **extra):
        config=self.request_config()
        config['provider']=provider
        config['base_url']=f'http://127.0.0.1:{self.provider.server_port}/v1'
        self.api('images/process',{'ids':ids,'action':action,'mode':mode,'service':config,**extra})
        self.finish()
        return self.api('images/state')

    def image_regions(self, texts):
        return [{'x':10,'y':10+i*35,'width':180,'height':30,'original':text,'translation':None} for i,text in enumerate(texts)]

    def test_text_duplicates_cache_and_restart_send_only_missing_unique_text(self):
        items=[None]+[{'name':'Save','description':'Help'} for _ in range(60)]
        (self.game/'data/Items.json').write_text(json.dumps(items),'utf-8')
        self.scan();self.assertEqual(self.api('state')['catalog']['entries'],2)
        self.translate();state=self.finish()
        self.assertEqual(state['progress']['translation_requests'],1)
        self.assertEqual(len(MockProvider.records),1)
        self.assertEqual(set(MockProvider.records[0]),{'Save','Help'})
        self.assertTrue(all(set(item)=={'id','text'} for item in MockProvider.payloads[0]))
        self.stop_app();self.start_app();self.scan()
        self.api('translate',self.request_config(),expected=400)
        another=self.root/'another-game';shutil.copytree(self.game,another)
        self.api('scan',{'path':str(another),'source_language':'en','target_language':'zh-CN'})
        self.translate();state=self.finish()
        self.assertEqual(state['progress']['cache_hits'],2)
        self.assertEqual(state['progress']['translation_requests'],0)
        self.assertEqual(len(MockProvider.records),1)

    def test_images_pool_deduplicates_across_images_even_without_memory_cache(self):
        assets=self.image_fixture(3)
        for i,a in enumerate(assets):
            self.api('images/edit',{'id':a['id'],'regions':self.image_regions(['Help',f'Item {i}'])})
        result=self.image_process([], 'auto',mode='local',scope='all',use_cache=False)
        self.assertEqual(result['progress']['text_regions'],6)
        self.assertEqual(result['progress']['unique_texts'],4)
        self.assertEqual(result['progress']['translation_requests'],1)
        self.assertEqual(result['progress']['ready'],3)
        self.assertTrue(self.api('state')['task_succeeded'])
        self.assertEqual(len(MockProvider.image_records),0)
        self.assertEqual(len(MockProvider.records),1)
        self.assertEqual(len(MockProvider.records[0]),4)
        self.assertEqual(len(set(MockProvider.records[0])),4)
        self.assertTrue(all(set(item)=={'id','text'} for item in MockProvider.payloads[0]))
        for a in result['project']['assets']:
            self.assertEqual(a['regions'][0]['translation'],'译文：Help')

    def test_image_partial_local_failure_does_not_show_success_after_previous_success(self):
        assets=self.image_fixture(2)
        for i,a in enumerate(assets):
            regions=self.image_regions(['Help'])
            regions[0]['translation']='帮助' if i==0 else None
            self.api('images/edit',{'id':a['id'],'regions':regions})
        self.image_process([assets[0]['id']],'render')
        self.assertTrue(self.api('state')['task_succeeded'])
        result=self.image_process([a['id'] for a in assets],'render')
        self.assertEqual(result['progress']['ready'],1)
        self.assertEqual(sum(a['error'] is not None for a in result['project']['assets']),1)
        self.assertFalse(self.api('state')['task_succeeded'])
        self.assertEqual(MockProvider.records,[])
        self.assertEqual(MockProvider.image_records,[])

    def test_images_pool_uses_persistent_text_cache_before_model_calls(self):
        assets=self.image_fixture(3)
        for a,texts in zip(assets,[['Help'],['Help','Options'],['Options','Quit']]):
            self.api('images/edit',{'id':a['id'],'regions':self.image_regions(texts)})
        self.image_process([assets[0]['id']],'translate')
        result=self.image_process([], 'auto',mode='local',scope='all')
        self.assertEqual(result['progress']['text_regions'],4)
        self.assertEqual(result['progress']['unique_texts'],3)
        self.assertEqual(result['progress']['cached_texts'],1)
        self.assertEqual(result['progress']['translation_requests'],1)
        self.assertEqual(result['progress']['ready'],3)
        self.assertEqual(MockProvider.records,[['Help'],['Options','Quit']])
        self.stop_app();self.start_app();self.scan()
        result=self.image_process([], 'auto',mode='local',scope='all')
        self.assertEqual(result['progress']['translation_requests'],0)
        self.assertEqual(len(MockProvider.records),2)
        self.assertEqual(len(MockProvider.image_records),0)

    def test_images_pool_pause_saves_one_batch_and_resumes_without_resending(self):
        assets=self.image_fixture(2)
        for a,texts in zip(assets,[['Help','Options'],['Help','Quit']]):
            self.api('images/edit',{'id':a['id'],'regions':self.image_regions(texts)})
        MockProvider.delay=0.5;MockProvider.received.clear()
        self.api('images/process',{'scope':'all','action':'auto','mode':'local','service':self.request_config(batch_size=1)})
        self.assertTrue(MockProvider.received.wait(5));self.api('images/stop',{});self.finish()
        self.assertFalse(self.api('state')['task_succeeded'])
        self.assertEqual(MockProvider.records,[['Help']])
        for a in self.api('images/state')['project']['assets']:
            self.assertEqual(a['regions'][0]['translation'],'译文：Help')
        self.stop_app();self.start_app();self.scan()
        result=self.image_process([], 'auto',mode='local',scope='all')
        self.assertEqual(result['progress']['ready'],2)
        self.assertTrue(self.api('state')['task_succeeded'])
        self.assertEqual(MockProvider.records,[['Help'],['Options','Quit']])

    def test_images_local_missing_language_stops_before_any_requests_or_project_writes(self):
        self.image_fixture(3)
        self.api('scan',{'path':str(self.game),'source_language':'zz','target_language':'zh-CN'})
        self.api('images/scan',{})
        before=self.api('images/state')['project']
        self.api('images/process',{'scope':'all','action':'auto','mode':'local','service':self.request_config()},expected=400)
        self.assertEqual(self.api('images/state')['project'],before)
        self.assertFalse(self.api('state')['busy'])
        self.assertEqual(MockProvider.records,[])
        self.assertEqual(MockProvider.image_records,[])

    def test_ocr_status_and_unsupported_install_do_not_call_models_or_write_projects(self):
        self.api('images/ocr', expected=400)
        self.scan()
        state=self.api('images/ocr')
        self.assertEqual(state['language'],'en')
        self.assertFalse(state['installable'])
        self.api('images/ocr/install',{},expected=400)
        self.assertFalse(self.api('state')['busy'])
        self.api('scan',{'path':str(self.game),'source_language':'ja','target_language':'zh-CN'})
        records=lambda:{str(p):p.read_bytes() for p in (self.root/'state').rglob('*.json')}
        before=records()
        for _ in range(3):
            state=self.api('images/ocr')
            self.assertEqual(state['language'],'ja')
            self.assertEqual(state['installable'],not state['available'])
            self.assertFalse(state['running'])
        self.assertEqual(self.api('images/state')['ocr'],state)
        self.assertEqual(before,records())
        self.assertEqual(MockProvider.records,[])
        self.assertEqual(MockProvider.image_records,[])

    def test_images_pool_failure_stops_new_requests_and_keeps_valid_batch(self):
        assets=self.image_fixture(2)
        for a,texts in zip(assets,[['Help','Options'],['Help','Quit']]):
            self.api('images/edit',{'id':a['id'],'regions':self.image_regions(texts)})
        MockProvider.fail_at=2
        self.api('images/process',{'scope':'all','action':'auto','mode':'local','service':self.request_config(batch_size=1)})
        self.finish();result=self.api('images/state')
        self.assertIn('处理停止',result['progress']['message'])
        self.assertFalse(self.api('state')['task_succeeded'])
        self.assertEqual(result['progress']['translation_requests'],2)
        self.assertEqual(MockProvider.records,[['Help'],['Options']])
        for a in result['project']['assets']:
            self.assertEqual(a['regions'][0]['translation'],'译文：Help')
            self.assertIsNone(a['regions'][1]['translation'])
        MockProvider.fail_at=0
        self.image_process([], 'auto',mode='local',scope='all')
        self.assertEqual(MockProvider.records,[['Help'],['Options'],['Options','Quit']])

    def test_images_vision_failure_stops_remaining_images(self):
        self.image_fixture(3);MockProvider.image_invalid=True
        result=self.image_process([], 'auto',scope='all',use_cache=False)
        self.assertIn('视觉识别失败',result['progress']['message'])
        self.assertFalse(self.api('state')['task_succeeded'])
        self.assertEqual(len(MockProvider.image_records),1)
        self.assertEqual(len(MockProvider.records),0)
        self.assertEqual(sum(a['error'] is not None for a in result['project']['assets']),1)

    def test_images_separate_flow_cache_restart_render_install_and_restore(self):
        assets=self.image_fixture();id=assets[0]['id']
        text_before=self.api('state')['catalog']
        for provider in ['openai','responses','anthropic']:
            image=self.image_process([id],'recognize',provider=provider)['project']['assets'][0]
            self.assertEqual(image['regions'][0]['original'],'Help')
        self.assertEqual(len(MockProvider.image_records),3)
        self.image_process([id],'recognize',provider='anthropic')
        self.assertEqual(len(MockProvider.image_records),3)
        image=self.image_process([id],'translate')['project']['assets'][0]
        self.assertEqual(image['regions'][0]['translation'],'译文：Help')
        image=self.image_process([id],'render')['project']['assets'][0]
        self.assertIsNotNone(image['rendered'])
        with urllib.request.urlopen(urllib.request.Request(self.base+'/api/images/preview?id='+id+'&kind=rendered',headers={'X-Session-Token':self.token})) as response:
            self.assertEqual(response.headers['Content-Type'],'image/png')
            rendered=response.read()
        self.stop_app();self.start_app();self.scan()
        self.assertEqual(self.api('images/state')['project']['assets'][0]['rendered'],image['rendered'])
        before=len(MockProvider.records);self.image_process([id],'translate');self.assertEqual(len(MockProvider.records),before)
        self.assertEqual(self.api('state')['catalog']['translated'],text_before['translated'])
        self.assertEqual(self.api('state')['catalog']['entries'],text_before['entries'])
        self.translate();self.finish();self.api('install',{})
        text_bytes=(self.game/'data/GameTranslatorTranslations.json').read_bytes()
        original=(self.game/'img/pictures/help0.png').read_bytes()
        self.api('images/install',{});self.assertEqual(self.api('images/state')['installation']['images'],1)
        self.api('images/trace',{'enabled':True});self.assertTrue(self.api('images/state')['installation']['record'])
        self.api('images/uninstall',{})
        self.assertEqual((self.game/'img/pictures/help0.png').read_bytes(),original)
        self.assertEqual((self.game/'data/GameTranslatorTranslations.json').read_bytes(),text_bytes)
        self.assertTrue(self.api('state')['catalog']['installation']['current'])
        self.api('images/import',{'id':id,'png':base64.b64encode(rendered).decode()})
        self.api('images/install',{});self.assertEqual(self.api('images/state')['installation']['images'],1)

    def test_images_invalid_recognition_preserves_previous_regions_and_selection_is_validated(self):
        assets=self.image_fixture();id=assets[0]['id']
        image=self.image_process([id],'recognize')['project']['assets'][0]
        MockProvider.image_invalid=True
        failed=self.image_process([id],'recognize',use_cache=False)['project']['assets'][0]
        self.assertEqual(failed['regions'],image['regions']);self.assertIsNotNone(failed['error'])
        self.api('images/process',{'ids':[],'action':'render'},expected=400)
        self.api('images/process',{'ids':['unknown'],'action':'render'},expected=400)
        self.api('images/process',{'ids':[id,id],'action':'render'},expected=400)
        region={**image['regions'][0],'x':99999}
        self.api('images/edit',{'id':id,'regions':[region]},expected=400)
        self.api('images/import',{'id':id,'png':'not png'},expected=400)
        self.assertEqual(self.api('images/state')['project']['assets'][0]['regions'],image['regions'])
        self.api('images/process',{'ids':[id],'action':'recognize','mode':'vision','service':{**self.request_config(),'provider':'deepl'}},expected=400)
        self.assertEqual(len(MockProvider.image_records),2)
        png=(self.game/'img/pictures/help0.png').read_bytes()
        (self.game/'img/pictures/help0.png_').write_bytes(png)
        self.api('images/scan',{})
        for candidate in self.api('images/state')['project']['assets']:
            self.api('images/import',{'id':candidate['id'],'png':base64.b64encode(png).decode()})
        self.api('images/install',{},expected=400)
        self.assertFalse((self.game/'.handy-images/install.json').exists())

    def test_images_stop_keeps_results_and_same_image_ocr_cache_is_reused(self):
        assets=self.image_fixture(2);ids=[a['id'] for a in assets]
        MockProvider.delay=0.5;MockProvider.received.clear()
        self.api('images/process',{'ids':ids,'action':'recognize','mode':'vision','service':self.request_config()})
        self.assertTrue(MockProvider.received.wait(5));self.api('images/stop',{});self.finish()
        state=self.api('images/state');self.assertEqual(state['progress']['processed'],1)
        self.stop_app();self.start_app();self.scan();self.image_process(ids,'recognize')
        self.assertEqual(len(MockProvider.image_records),1)
        self.assertTrue(all(a['recognized'] for a in self.api('images/state')['project']['assets']))
        self.assertEqual(self.api('state')['catalog']['translated'],0)

    def test_images_auto_skips_empty_and_reuses_empty_results_after_restart(self):
        assets=self.image_fixture(2)
        MockProvider.image_empty=True
        result=self.image_process([], 'auto', scope='all')
        self.assertEqual(result['progress']['total'],2)
        self.assertEqual(result['progress']['skipped'],2)
        self.assertEqual(len(MockProvider.image_records),1)
        self.assertEqual(len(MockProvider.records),0)
        self.assertTrue(all(a['recognized'] and not a['regions'] and not a['rendered'] and not a['error'] for a in result['project']['assets']))
        self.image_process([assets[0]['id']], 'translate')
        self.assertIsNone(self.api('images/state')['project']['assets'][0]['error'])
        self.stop_app();self.start_app();self.scan()
        result=self.image_process([], 'auto', scope='all',use_cache=False)
        self.assertEqual(result['progress']['skipped'],2)
        self.assertEqual(len(MockProvider.image_records),1)
        self.assertEqual(len(MockProvider.records),0)

    def test_images_auto_processes_more_than_100_and_keeps_completed_output(self):
        self.image_fixture(103)
        MockProvider.delay=0
        result=self.image_process([], 'auto', scope='all')
        self.assertEqual(result['progress']['total'],103)
        self.assertEqual(result['progress']['ready'],103)
        self.assertTrue(all(a['rendered'] and not a['error'] for a in result['project']['assets']))
        self.assertEqual(len(MockProvider.image_records),1)
        self.assertEqual(len(MockProvider.records),1)
        before=result['project']['assets']
        result=self.image_process([], 'auto', scope='all',use_cache=False)
        self.assertEqual(result['project']['assets'],before)
        self.assertEqual(len(MockProvider.image_records),1)
        self.assertEqual(len(MockProvider.records),1)
        self.api('images/process',{'scope':'all','action':'render'},expected=400)
        self.api('images/process',{'scope':'all','ids':[before[0]['id']],'action':'auto','mode':'vision','service':self.request_config()},expected=400)
        self.api('images/process',{'scope':'unknown','action':'auto'},expected=400)

    def test_images_auto_pause_saves_recognition_and_resumes_without_resending(self):
        self.image_fixture(2)
        MockProvider.delay=0.5;MockProvider.received.clear()
        self.api('images/process',{'scope':'all','action':'auto','mode':'vision','service':self.request_config()})
        self.assertTrue(MockProvider.received.wait(5));self.api('images/stop',{});self.finish()
        first=self.api('images/state')['project']['assets'][0]
        self.assertIsNotNone(first['recognized'])
        self.assertIsNone(first['rendered'])
        self.assertEqual(len(MockProvider.records),0)
        self.stop_app();self.start_app();self.scan()
        result=self.image_process([], 'auto',scope='all')
        self.assertEqual(result['progress']['ready'],2)
        self.assertEqual(len(MockProvider.image_records),1)
        self.assertEqual(len(MockProvider.records),1)

    def test_restore_installed_fills_missing_preserves_edits_and_recovers_fresh_directory(self):
        self.scan();self.translate();self.finish();self.api('install',{})
        bundle=self.api('export')
        rows=self.api('entries')['entries']
        self.api('entry',{'id':rows[0]['id'],'translation':None})
        custom='手动译文 '+rows[1]['translation']
        self.api('entry',{'id':rows[1]['id'],'translation':custom})
        self.assertEqual(self.api('restore-installed',{})['restored'],1)
        actual=self.api('export')['translations']
        self.assertEqual(actual[rows[0]['original']],rows[0]['translation'])
        self.assertEqual(actual[rows[1]['original']],custom)
        self.assertEqual(len(MockProvider.records),1)
        self.scan('ko');self.api('restore-installed',{},expected=400)
        self.assertEqual(self.api('state')['catalog']['translated'],0)
        self.stop_app();(self.root/'state').rename(self.root/'previous-state')
        self.start_app();self.scan()
        self.assertEqual(self.api('export')['translations'],bundle['translations'])
        self.assertIn('从游戏恢复',self.api('state')['progress']['message'])
        self.assertTrue(self.api('state')['catalog']['installation']['current'])
        self.assertEqual(len(MockProvider.records),1)
        path=self.game/'data/GameTranslatorTranslations.json'
        path.write_text('{}','utf-8')
        self.api('restore-installed',{},expected=400)
        self.assertEqual(self.api('export')['translations'],bundle['translations'])

    def test_cache_damage_does_not_destroy_projects_and_can_be_bypassed(self):
        self.scan(); self.translate(); self.finish()
        another = self.root/'another-game'; shutil.copytree(self.game,another)
        self.api('scan',{'path':str(another),'source_language':'en','target_language':'zh-CN'})
        memory = self.root/'state/translation-memory.json'
        memory.write_text('invalid cache','utf-8')
        self.api('translate',self.request_config())
        state = self.finish()
        self.assertEqual(state['catalog']['translated'],0)
        self.assertIn('缓存无法读取',state['progress']['message'])
        self.api('translate',self.request_config(use_cache=False))
        self.assertEqual(self.finish()['catalog']['translated'],26)
        self.assertEqual(memory.read_text('utf-8'),'invalid cache')
        self.stop_app(); self.start_app(); self.api('scan',{'path':str(another),'source_language':'en','target_language':'zh-CN'})
        self.assertEqual(self.api('state')['catalog']['translated'],26)
        self.assertIn('缓存未更新',self.api('state')['catalog']['warnings'][-1])


if __name__ == '__main__':
    unittest.main(verbosity=2)
