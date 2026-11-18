/*:
 * @plugindesc 加载Handy-translate生成的本地译文。
 * @help 通过Handy-translate安装与卸载。更新译文后需要完全重启游戏。
 */
(function () {
    'use strict';
    window.$gameTranslatorData = null;
    DataManager._databaseFiles.push({ name: '$gameTranslatorData', src: 'GameTranslatorTranslations.json' });

    function translate(text) {
        if (typeof text !== 'string') return text;
        var data = window.$gameTranslatorData;
        var map = data && data.translations;
        if (!map) return text;
        if (Object.prototype.hasOwnProperty.call(map, text)) return map[text];
        return text.split('\n').map(function (line) {
            return Object.prototype.hasOwnProperty.call(map, line) ? map[line] : line;
        }).join('\n');
    }

    var convert = Window_Base.prototype.convertEscapeCharacters;
    Window_Base.prototype.convertEscapeCharacters = function (text) {
        return convert.call(this, translate(text));
    };
    var renderDepth = 0;
    // 富文本已经整句翻译，逐字绘制和测量时不能再次匹配单字原文。
    ['processNormalCharacter', 'flushTextState'].forEach(function (name) {
        var original = Window_Base.prototype[name];
        if (!original) return;
        Window_Base.prototype[name] = function () {
            renderDepth++;
            try { return original.apply(this, arguments); }
            finally { renderDepth--; }
        };
    });
    var draw = Bitmap.prototype.drawText;
    Bitmap.prototype.drawText = function (text, x, y, width, height, align) {
        return draw.call(this, renderDepth ? text : translate(text), x, y, width, height, align);
    };
    var measure = Bitmap.prototype.measureTextWidth;
    Bitmap.prototype.measureTextWidth = function (text) {
        return measure.call(this, renderDepth ? text : translate(text));
    };

    var format = String.prototype.format;
    if (format) {
        String.prototype.format = function () {
            var source = this.toString();
            var translated = translate(source);
            if (translated === source) return format.apply(this, arguments);
            var args = Array.prototype.map.call(arguments, translate);
            return format.apply(translated, args);
        };
    }
}());
