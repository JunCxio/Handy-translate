const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = fs.readFileSync(path.join(__dirname,'../adapters/HandyImageTranslator.js'),'utf8');
function setup() {
  const calls = [], writes = [], timers = [];
  function Bitmap() {this._image={};} Bitmap.prototype._startLoading=function(){};
  Bitmap.prototype._startDecrypting=function(){this.original=true;};
  const context=vm.createContext({window:{},DataManager:{_databaseFiles:[]},Utils:{isNwjs:()=>true},ImageManager:{loadBitmap(...args){calls.push(args);return args;}},Bitmap,Decrypter:{checkImgIgnore:url=>url==='original.png'},process:{mainModule:{filename:'E:\\Game\\index.html'}},setTimeout:fn=>{timers.push(fn);return 1;},require:name=>name==='fs'?{existsSync:()=>true,readFileSync:()=>JSON.stringify(['img/pictures/previous.png']),writeFileSync:(...args)=>writes.push(args),renameSync:(...args)=>writes.push(args)}:path.win32,console});
  vm.runInContext(source,context);
  return {context,calls,writes,timers,Bitmap};
}
test('无译图时保持原加载，替换仅接受受管理路径并保留其他参数',()=>{
  const {context:c,calls}=setup();
  c.ImageManager.loadBitmap('img/pictures/','help',0,true);assert.deepEqual(calls[0],['img/pictures/','help',0,true]);
  const file='.handy-images/files/'+'a'.repeat(64)+'.png';c.window.$handyImageTranslations={images:{'img/pictures/help.png':file}};
  c.ImageManager.loadBitmap('img/pictures/','help',25,false);assert.deepEqual(calls[1],['.handy-images/files/','a'.repeat(64),25,false]);
  c.window.$handyImageTranslations.images['img/pictures/help.png']='../evil.png';c.ImageManager.loadBitmap('img/pictures/','help');assert.equal(calls[2][0],'img/pictures/');
  assert.equal(c.DataManager._databaseFiles[0].src,'HandyImageTranslations.json');
});
test('加密游戏仅为受管理 PNG 绕过解密，MV 和 MZ 原素材行为不变',()=>{
  const {context:c,Bitmap}=setup();const own='.handy-images/files/'+'b'.repeat(64)+'.png';
  const original=new Bitmap();original._url='img/pictures/help.png';original._startDecrypting();assert.equal(original.original,true);
  const replacement=new Bitmap();replacement._url=own;replacement._startDecrypting();assert.equal(replacement.original,undefined);assert.equal(replacement._image.src,own);
  assert.equal(c.Decrypter.checkImgIgnore(own),true);assert.equal(c.Decrypter.checkImgIgnore('original.png'),true);assert.equal(c.Decrypter.checkImgIgnore('img/pictures/help.png'),false);
});
test('只有显式开启记录才写固定日志，并合并之前加载记录',()=>{
  const {context:c,writes,timers}=setup();c.window.$handyImageTranslations={record:false,images:{}};c.ImageManager.loadBitmap('img/pictures/','off');assert.equal(timers.length,0);
  c.window.$handyImageTranslations.record=true;c.ImageManager.loadBitmap('img/pictures/','help');c.ImageManager.loadBitmap('img/pictures/','another');assert.equal(timers.length,1);timers[0]();
  assert.equal(writes[0][0],'E:\\Game\\.handy-images\\loaded.json.tmp');assert.deepEqual(JSON.parse(writes[0][1]),['img/pictures/another.png','img/pictures/help.png','img/pictures/previous.png']);
});
