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
function syncMemberRow(box) {
  const g = groupBoxOf(box);
  if (g && g.checked && !box.checked) {
    g.checked = false;
    box.closest('tr.members').querySelectorAll('input.row').forEach(b => { if (b !== box) b.checked = true; });
  }
}
document.addEventListener('click', e => {
  const c = e.target;
  if (!c.matches || !c.matches('input.row')) return;
  const boxes = [...document.querySelectorAll('input.row')];
  const a = boxes.indexOf(lastBox), b = boxes.indexOf(c);
  if (e.shiftKey && a >= 0) {
    for (let i = Math.min(a, b); i <= Math.max(a, b); i++) {
      boxes[i].checked = c.checked;
      if (boxes[i].classList.contains('grp')) {
        boxes[i].closest('tr').nextElementSibling.querySelectorAll('input.row').forEach(m => m.checked = c.checked);
      } else if (boxes[i].closest('tr.members')) {
        syncMemberRow(boxes[i]);
      }
    }
  }
  lastBox = c;
  countSelected();
});
function toggleAll(c) {
  document.querySelectorAll('input.row').forEach(b => b.checked = c.checked);
  countSelected();
}
// A ticked group counts all its files, whether or not they are shown.
function countSelected() {
  let n = 0;
  document.querySelectorAll('input.row:checked').forEach(b => {
    if (b.classList.contains('grp')) n += +b.dataset.n;
    else if (!memberOfTickedGroup(b)) n += 1;
  });
  if (el('selcount')) el('selcount').textContent = n + ' selected';
}
function groupBoxOf(box) {
  const m = box.closest('tr.members');
  return m ? m.previousElementSibling.querySelector('input.grp') : null;
}
function memberOfTickedGroup(box) {
  const g = groupBoxOf(box);
  return g && g.checked;
}

// Images, grouped: a group row is followed by a hidden row for its files,
// which are fetched the first time it is opened.
function toggleGroup(tr, open) {
  const m = tr.nextElementSibling;
  if (open === undefined) open = m.hidden;
  m.hidden = !open;
  tr.querySelector('.arrow').textContent = open ? '▾' : '▸';
  if (open) htmx.trigger(m.lastElementChild, 'expand');
}
function expandAll(open) {
  document.querySelectorAll('tr.group').forEach(tr => toggleGroup(tr, open));
}
function toggleGroupBox(g) {
  g.closest('tr').nextElementSibling.querySelectorAll('input.row').forEach(b => b.checked = g.checked);
  countSelected();
}
// Unticking one file of a ticked group leaves the rest of it selected.
document.addEventListener('change', e => {
  const c = e.target;
  if (!c.matches || !c.matches('tr.members input.row')) return;
  syncMemberRow(c);
  countSelected();
});
// Keep the job notes drawer open across #jobs polls.
let notesOpen = false;
document.addEventListener('htmx:beforeSwap', e => {
  if (e.target && e.target.id === 'jobs') {
    const d = e.target.querySelector('details.notes');
    notesOpen = Boolean(d && d.open);
  }
});
document.addEventListener('htmx:afterSwap', e => {
  if (e.target && e.target.id === 'jobs' && notesOpen) {
    const d = e.target.querySelector('details.notes');
    if (d) d.open = true;
  }
});
// Files of a ticked group arrive ticked.
document.addEventListener('htmx:afterSwap', e => {
  const m = e.target.closest && e.target.closest('tr.members');
  const g = m && m.previousElementSibling.querySelector('input.grp');
  if (g && g.checked) m.querySelectorAll('input.row').forEach(b => b.checked = true);
});
// Deleting from disk can't be undone: ask for the word, not just a click.
function confirmDelete() {
  if (!needSelection()) return false;
  const n = el('all').checked ? 'ALL matching' : parseInt(el('selcount').textContent);
  const typed = prompt('Permanently delete ' + n + ' files from disk?\n\nThis cannot be undone. Type DELETE to confirm.');
  if (typed === null) return false;
  if (typed.trim() !== 'DELETE') { alert('Nothing deleted: you did not type DELETE.'); return false; }
  el('confirm').value = 'DELETE';
  return true;
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
  if (e.target.id === 'sql') return reload(0);
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

// On a phone the tabs are one scrolling row: bring the current one into view.
window.addEventListener('load', () => {
  const nav = document.querySelector('nav'), a = document.querySelector('nav a.active');
  if (a) nav.scrollLeft = a.offsetLeft - (nav.clientWidth - a.offsetWidth) / 2;
});
