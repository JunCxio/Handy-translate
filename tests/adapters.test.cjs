'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const context = vm.createContext({});
vm.runInContext(`
  var window = this;
  var DataManager = { _databaseFiles: [] };
  function Bitmap() {}
  Bitmap.prototype.drawText = function(text) { return text; };
  Bitmap.prototype.measureTextWidth = function(text) { return text.length; };
  function Window_Base() {}
  Window_Base.prototype.convertEscapeCharacters = function(text) { return text.replace(/\\\\V\\[1\\]/g, '42'); };
  Window_Base.prototype.drawText = function(text) { return new Bitmap().drawText(text); };
  Window_Base.prototype.processNormalCharacter = function(text) { return new Bitmap().drawText(text); };
  Window_Base.prototype.flushTextState = function(text) { return new Bitmap().drawText(text); };
  String.prototype.format = function() { var args = arguments; return this.replace(/%([0-9]+)/g, function(_, n) { return args[Number(n) - 1]; }); };
`, context);
vm.runInContext(fs.readFileSync(path.join(__dirname, '../adapters/GameTranslator.js'), 'utf8'), context);
vm.runInContext(`
  $gameTranslatorData = { translations: { 'Hello \\\\V[1]': '你好 \\\\V[1]', 'Potion': '药水', '%1 wins': '%1 获胜', 'Hero': '勇者', 'A': '甲' } };
`, context);
assert.equal(vm.runInContext(`new Window_Base().convertEscapeCharacters('Hello \\\\V[1]\\nPotion')`, context), '你好 42\n药水');
assert.equal(vm.runInContext(`new Window_Base().drawText('Potion')`, context), '药水');
assert.equal(vm.runInContext(`new Bitmap().drawText('Potion')`, context), '药水');
assert.equal(vm.runInContext(`new Bitmap().measureTextWidth('Potion')`, context), 2);
assert.equal(vm.runInContext(`new Window_Base().processNormalCharacter('A')`, context), 'A');
assert.equal(vm.runInContext(`new Window_Base().flushTextState('A')`, context), 'A');
assert.equal(vm.runInContext(`new Window_Base().convertEscapeCharacters('A Potion')`, context), 'A Potion');
assert.equal(vm.runInContext(`'%1 wins'.format('Hero')`, context), '勇者 获胜');
assert.equal(vm.runInContext(`'%1.png'.format('Hero')`, context), 'Hero.png');
assert.equal(vm.runInContext(`DataManager._databaseFiles[0].src`, context), 'GameTranslatorTranslations.json');
vm.runInContext(`$gameTranslatorData = null`, context);
assert.equal(vm.runInContext(`new Window_Base().drawText('Potion')`, context), 'Potion');
console.log('RPG Maker adapter: 11 checks passed');
