import io
import json
import textwrap
import types
import unittest
from pathlib import Path


class RenpyAdapterTest(unittest.TestCase):
    def test_translates_before_substitution_and_keeps_original_for_refresh(self):
        class Text:
            locked = False
            tokenized = False

            def set_text(self, text, scope=None, substitute=False, update=True):
                self.text_parameter = text
                self.text = [part.replace('[name]', scope['name']) if isinstance(part, str) and substitute else part for part in text]
                return True

        data = {'translations': {'Hello [name]': '你好 [name]', '你好 [name]': '不应二次翻译'}}
        renpy = types.SimpleNamespace(
            file=lambda _: io.BytesIO(json.dumps(data).encode('utf-8')),
            log=lambda message: self.fail(message),
            text=types.SimpleNamespace(text=types.SimpleNamespace(Text=Text)),
        )
        source = Path(__file__).parents[1].joinpath('adapters/zzz_game_translator.rpy').read_text('utf-8')
        namespace = {'renpy': renpy}
        exec(compile(textwrap.dedent(source.split('\n', 1)[1]), 'adapter', 'exec'), namespace)
        item = Text()
        item.set_text('Hello [name]', {'name': '小明'}, True)
        self.assertEqual(item.text, ['你好 小明'])
        self.assertEqual(item.text_parameter, ['Hello [name]'])
        item.set_text(item.text_parameter, {'name': '小红'}, True)
        self.assertEqual(item.text, ['你好 小红'])
        displayable = object()
        item.set_text([displayable, 'unknown'], {}, False)
        self.assertIs(item.text[0], displayable)
        self.assertEqual(item.text[1], 'unknown')
        item.tokenized = True
        item.set_text([('TEXT', 'Hello [name]')])
        self.assertEqual(item.text, [('TEXT', 'Hello [name]')])


if __name__ == '__main__':
    unittest.main()
