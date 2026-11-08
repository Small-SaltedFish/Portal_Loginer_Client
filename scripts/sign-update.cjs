const fs=require('node:fs');const crypto=require('node:crypto');const path=require('node:path');
const root=path.resolve(__dirname,'..');
if(process.argv[2]==='keygen'){
 const privatePath=path.join(root,'.keys/update-signing.pem');const publicPath=path.join(root,'assets/update-public-key.bin');
 if(fs.existsSync(privatePath)||fs.existsSync(publicPath))throw Error('Signing keys already exist; refusing to replace them');
 const pair=crypto.generateKeyPairSync('ed25519');fs.writeFileSync(privatePath,pair.privateKey.export({type:'pkcs8',format:'pem'}),{mode:0o600,flag:'wx'});
 fs.writeFileSync(publicPath,pair.publicKey.export({type:'spki',format:'der'}).subarray(-32),{flag:'wx'});console.log('Created signing key and public key; private key stays in .keys');
}else if(process.argv[2]==='verify'){
 const pub=fs.readFileSync(path.join(root,'assets/update-public-key.bin'));
 const key=crypto.createPublicKey({key:Buffer.concat([Buffer.from('302a300506032b6570032100','hex'),pub]),type:'spki',format:'der'});
 if(!crypto.verify(null,fs.readFileSync(process.argv[3]),key,fs.readFileSync(process.argv[4])))throw Error('Manifest signature mismatch');
 const manifest=JSON.parse(fs.readFileSync(process.argv[3],'utf8'));for(const asset of manifest.assets){const body=fs.readFileSync(path.join(path.dirname(process.argv[3]),asset.name));if(body.length!==asset.size||crypto.createHash('sha256').update(body).digest('hex')!==asset.sha256)throw Error('Payload integrity mismatch');}console.log('Manifest and payload verified');
}else{
 const privatePem=process.env.PORTAL_UPDATE_SIGNING_KEY||fs.readFileSync(path.join(root,'.keys/update-signing.pem'),'utf8');
 const key=crypto.createPrivateKey(privatePem);const pub=crypto.createPublicKey(key).export({type:'spki',format:'der'}).subarray(-32);
 if(!pub.equals(fs.readFileSync(path.join(root,'assets/update-public-key.bin'))))throw Error('Signing key does not match embedded public key');
 const manifest=fs.readFileSync(process.argv[2]);fs.writeFileSync(process.argv[3],crypto.sign(null,manifest,key));console.log('Signed update manifest');
}
