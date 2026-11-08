const fs=require('node:fs'), crypto=require('node:crypto'),path=require('node:path');
const file=process.argv[2], manifest=JSON.parse(fs.readFileSync(file,'utf8'));
if(manifest.repository!=='Small-SaltedFish/Portal_Loginer_Client')throw Error('Wrong repository');
for(const asset of manifest.assets){
 if(path.basename(asset.name)!==asset.name)throw Error('Invalid asset name');
 const body=fs.readFileSync(path.join(path.dirname(file),asset.name));
 if(body.length!==asset.size||crypto.createHash('sha256').update(body).digest('hex')!==asset.sha256)throw Error('Payload integrity mismatch');
}
console.log('Payload size and SHA-256 verified');
