init 999 python:
    import json as _gt_json

    try:
        _gt_file = renpy.file("GameTranslatorTranslations.json")
        try:
            _gt_map = _gt_json.loads(_gt_file.read().decode("utf-8"))["translations"]
        finally:
            _gt_file.close()
    except Exception as _gt_error:
        renpy.log("GameTranslator: cannot load translations: " + str(_gt_error))
        _gt_map = {}

    try:
        _gt_string_type = basestring
    except NameError:
        _gt_string_type = str

    def _gt_translate(value):
        if isinstance(value, _gt_string_type):
            return _gt_map.get(value, value)
        return value

    _gt_original_set_text = renpy.text.text.Text.set_text

    def _gt_set_text(self, text, *args, **kwargs):
        if getattr(self, "locked", False) or getattr(self, "tokenized", False):
            return _gt_original_set_text(self, text, *args, **kwargs)
        original = text if isinstance(text, list) else [text]
        translated = [_gt_translate(part) for part in original]
        result = _gt_original_set_text(self, translated, *args, **kwargs)
        # 保留原始模板，后续界面刷新仍先翻译再插入变量。
        self.text_parameter = original
        return result

    renpy.text.text.Text.set_text = _gt_set_text
