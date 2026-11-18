/*:
 * @plugindesc Handy-translate 独立图片替换与加载记录。
 * @help 通过图片素材模块安装、更新或还原，修改后完全重启游戏。
 */
(function () {
    'use strict';
    window.$handyImageTranslations = null;
    DataManager._databaseFiles.push({ name: '$handyImageTranslations', src: 'HandyImageTranslations.json' });
    var loaded = Object.create(null), timer = null;
    function record(url) {
        var data = window.$handyImageTranslations;
        if (!data || !data.record || !Utils.isNwjs() || loaded[url]) return;
        loaded[url] = true;
        if (timer) return;
        timer = setTimeout(function () {
            timer = null;
            try {
                var fs = require('fs'), path = require('path');
                var root = path.dirname(process.mainModule.filename);
                var file = path.join(root, '.handy-images', 'loaded.json');
                var previous = [];
                if (fs.existsSync(file)) previous = JSON.parse(fs.readFileSync(file, 'utf8'));
                if (!Array.isArray(previous)) throw new Error('素材记录格式无效');
                previous.forEach(function (p) { if (typeof p === 'string') loaded[p] = true; });
                var items = Object.keys(loaded).sort().slice(0, 20000);
                fs.writeFileSync(file + '.tmp', JSON.stringify(items));
                fs.renameSync(file + '.tmp', file);
            } catch (error) { console.warn('Handy-translate 素材记录未保存', error); }
        }, 500);
    }
    var load = ImageManager.loadBitmap;
    ImageManager.loadBitmap = function (folder, filename) {
        if (!filename) return load.apply(this, arguments);
        var url = folder + filename + '.png';
        record(url);
        var data = window.$handyImageTranslations;
        var replacement = data && data.images && Object.prototype.hasOwnProperty.call(data.images, url) && data.images[url];
        if (typeof replacement !== 'string' || !/^\.handy-images\/files\/[a-f0-9]{64}\.png$/.test(replacement)) return load.apply(this, arguments);
        var args = Array.prototype.slice.call(arguments);
        args[0] = '.handy-images/files/';
        args[1] = replacement.slice(args[0].length, -4);
        return load.apply(this, args);
    };
    function own(url) { return typeof url === 'string' && /^\.handy-images\/files\/[a-f0-9]{64}\.png$/.test(url); }
    // 译图为普通 PNG，只对本插件管理的路径绕过游戏的全局图片加密。
    if (Bitmap.prototype._startDecrypting && Bitmap.prototype._startLoading) {
        var decrypt = Bitmap.prototype._startDecrypting;
        Bitmap.prototype._startDecrypting = function () {
            if (!own(this._url)) return decrypt.apply(this, arguments);
            this._image.src = this._url;
        };
    }
    if (typeof Decrypter !== 'undefined' && Decrypter.checkImgIgnore) {
        var ignore = Decrypter.checkImgIgnore;
        Decrypter.checkImgIgnore = function (url) { return own(url) || ignore.apply(this, arguments); };
    }
}());
