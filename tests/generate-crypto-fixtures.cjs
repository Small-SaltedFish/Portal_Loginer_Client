// Independent oracle matching tests/baseline-audit.sh. Synthetic inputs only.
const fs = require('node:fs');
const crypto = require('node:crypto');
const key = '5a3b9f207411a8ed';
const prefix = 'xK9z';
const passwords = ['testpass', '123456789012', '1234567890123',
  'ABCDEFGHIJKLMNOPQRSTUVWXYZ012345', '1234567890123456789012345678901!'];
const cases = passwords.map(password => {
  let encrypted = password;
  if (!/^[A-Za-z0-9]{32}$/.test(password)) {
    const bytes = Buffer.from(prefix + password, 'utf8');
    const padded = Buffer.alloc(Math.ceil(bytes.length / 16) * 16);
    bytes.copy(padded);
    const cipher = crypto.createCipheriv('aes-128-ecb', Buffer.from(key), null);
    cipher.setAutoPadding(false);
    encrypted = Buffer.concat([cipher.update(padded), cipher.final()]).toString('hex');
  }
  return {password, encrypted};
});
fs.writeFileSync('fixtures/crypto.json', JSON.stringify({key, prefix, cases}, null, 2) + '\n');
