function el(id) { return document.getElementById(id); }
function dlg(id) { el(id).showModal(); }

// Images: search, sort and paging reload the table only.
function reload(page) {
  if (page !== undefined) el('page').value = page;
  htmx.trigger('#rows', 'reload');
}
function setSort(key) {
  const s = el('sort'), d = el('desc');
  if (s.value === key) d.value = d.value ? '' : '1';
  else { s.value = key; d.value = key === 'date' ? '1' : ''; }
  reload(0);
}
function setSearch(text) { el('q').value = text; reload(0); }

// Images: selection. Shift-click ticks the rows in between.
let lastBox = null;
document.addEventListener('click', e => {
  const c = e.target;
  if (!c.matches || !c.matches('input.row')) return;
  const boxes = [...document.querySelectorAll('input.row')];
  const a = boxes.indexOf(lastBox), b = boxes.indexOf(c);
  if (e.shiftKey && a >= 0) {
    for (let i = Math.min(a, b); i <= Math.max(a, b); i++) boxes[i].checked = c.checked;
  }
  lastBox = c;
  countSelected();
});
function toggleAll(c) {
  document.querySelectorAll('input.row').forEach(b => b.checked = c.checked);
  countSelected();
}
function countSelected() {
  const n = document.querySelectorAll('input.row:checked').length;
  if (el('selcount')) el('selcount').textContent = n + ' selected';
}
function needSelection() {
  if (el('all').checked || document.querySelector('input.row:checked')) return true;
  alert('Select some files first.');
  return false;
}
document.addEventListener('htmx:afterSwap', countSelected);
// Enter in a field of the Images form would press its first button (Remove
// from catalogue); in a dialog it means that dialog's own button instead.
document.addEventListener('keydown', e => {
  if (e.key !== 'Enter' || !e.target.matches || !e.target.matches('#sel input')) return;
  e.preventDefault();
  const d = e.target.closest('dialog');
  if (d) d.querySelector('button[formaction]').click();
});

// The clipboard API needs https; the old way works on a plain LAN address.
function copyText(text) {
  const t = document.createElement('textarea');
  t.value = text;
  document.body.appendChild(t);
  t.select();
  document.execCommand('copy');
  t.remove();
}

// Folder picker.
function browse(target) {
  htmx.ajax('GET', '/browse?target=' + target + '&path=' + encodeURIComponent(el(target).value),
    '#browse-body');
  el('browse').showModal();
}
function pick(target, path) { el(target).value = path; el('browse').close(); }
