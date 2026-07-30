// Injected into https://music.apple.com, which runs hidden as our audio daemon.
//
// DELIBERATELY DUMB. This file is the one part of the system that cannot be
// unit tested, so it contains no decisions it marshals MusicKit calls and
// events, nothing more. Anything resembling logic belongs in Rust.
//
// Injection happens via onPageStarted on remote URLs and is not guaranteed to
// run before the page's own scripts, so we poll for MusicKit rather than
// assuming it exists.
(function () {
  'use strict';
  if (window.top !== window.self) return;
  if (window.__saint) return;

  var saint = { ready: false, music: null };
  window.__saint = saint;

  function rawInvoke(cmd, args) {
    try {
      var internals = window.__TAURI_INTERNALS__;
      if (!internals || !internals.invoke) return;
      var p = internals.invoke(cmd, args || {});
      if (p && p.then) {
        p.then(null, function (e) {
          console.error('[saint] rejected', cmd, e);
        });
      }
    } catch (e) {
      console.error('[saint] invoke threw', cmd, e);
    }
  }

  function send(cmd, payload) {
    rawInvoke(cmd, { payload: payload });
  }

  function log(msg) {
    rawInvoke('engine_log', { msg: String(msg) });
  }

  function emit(kind, data) {
    send('engine_event', { kind: kind, data: data === undefined ? null : data });
  }

  function reportTokens(inst) {
    send('engine_tokens', {
      dev: inst.developerToken || '',
      user: inst.musicUserToken || '',
      storefront: inst.storefrontId || ''
    });
  }

  function wire(MK, inst) {
    var E = (MK && MK.Events) || {};

    inst.addEventListener(E.playbackStateDidChange || 'playbackStateDidChange', function () {
      if (saint.__prewarming) return;
      emit('playbackState', { state: inst.playbackState });
    });

    inst.addEventListener(E.nowPlayingItemDidChange || 'nowPlayingItemDidChange', function () {
      if (saint.__prewarming) return;
      var it = inst.nowPlayingItem;
      emit('nowPlaying', it ? {
        id: String(it.id || ''),
        title: it.title || '',
        artist: it.artistName || '',
        album: it.albumName || '',
        durationMs: it.playbackDuration || 0
      } : null);
    });

    var lastSecond = -1;
    inst.addEventListener(E.playbackTimeDidChange || 'playbackTimeDidChange', function () {
      if (saint.__prewarming) return;
      var t = inst.currentPlaybackTime || 0;
      var s = Math.floor(t);
      if (s === lastSecond) return;
      lastSecond = s;
      emit('position', { ms: Math.round(t * 1000) });
    });

    inst.addEventListener(E.authorizationStatusDidChange || 'authorizationStatusDidChange', function () {
      emit('authorization', { authorized: !!inst.isAuthorized });
    });
  }

  var tries = 0;
  var timer = setInterval(function () {
    tries++;
    var MK = window.MusicKit;
    var inst = null;
    try {
      inst = MK && MK.getInstance ? MK.getInstance() : null;
    } catch (e) {
      inst = null;
    }

    if (!inst) {
      if (tries === 20) log('still waiting for MusicKit after 20s');
      if (tries > 90) {
        clearInterval(timer);
        send('engine_ready', { ok: false, reason: 'MusicKit not found after 90s' });
      }
      return;
    }

    if (!inst.isAuthorized) {
      if (tries % 5 === 0) send('engine_ready', { ok: false, reason: 'unauthorized' });
      return;
    }

    clearInterval(timer);
    saint.music = inst;
    saint.ready = true;
    wire(MK, inst);
    reportTokens(inst);

    send('engine_ready', { ok: true, reason: '' });
  }, 1000);

  function unresolvableIds(message) {
    var m = /could not be resolved:\s*([\d,\s]+)/.exec(String(message || ''));
    if (!m) return [];
    return m[1].split(',').map(function (s) { return s.trim(); }).filter(Boolean);
  }

  var pendingQueue = Promise.resolve();
  var generation = 0;
  var pending = { gen: 0, rest: null };

  function withoutDeadIds(label, ids, run) {
    return run(ids).catch(function (e) {
      var bad = unresolvableIds(e && e.message);
      if (!bad.length) throw e;
      emit('unresolvable', { ids: bad });
      var kept = ids.filter(function (id) { return bad.indexOf(String(id)) === -1; });
      if (!kept.length) throw e;
      log(label + ': dropped ' + bad.length + ' unresolvable id(s), retrying with ' + kept.length);
      return run(kept);
    });
  }

  saint.setQueue = function (ids, startIndex) {
    if (!saint.music) return;
    var m = saint.music;
    var t0 = Date.now();
    saint.__prewarming = false;

    var start = Math.min(startIndex || 0, Math.max(0, ids.length - 1));
    var first = ids[start];
    var mine = { gen: ++generation, rest: ids.slice(start + 1) };
    pending = mine;

    log('setQueue: START 1 of ' + ids.length + ' (rest deferred)');
    pendingQueue = withoutDeadIds('setQueue', [first], function (list) {
      return m.setQueue({ songs: list, startWith: 0 });
    })
      .then(function () { log('setQueue: DONE in ' + (Date.now() - t0) + 'ms'); })
      .catch(function (e) {
        mine.rest = null;
        log('setQueue: FAILED after ' + (Date.now() - t0) + 'ms');
        emit('error', { op: 'setQueue', message: String((e && e.message) || e) });
        throw e;
      });

    pendingQueue.catch(function () {});
  };

  function appendRest(m, mine) {
    if (mine.gen !== generation) {
      log('playLater: skipped, superseded by a newer play');
      return;
    }
    var rest = mine.rest;
    mine.rest = null;
    if (!rest || !rest.length) return;
    var t0 = Date.now();
    withoutDeadIds('playLater', rest, function (list) {
      return m.playLater({ songs: list });
    })
      .then(function () {
        log('playLater: appended ' + rest.length + ' in ' + (Date.now() - t0) + 'ms');
      })
      .catch(function (e) {
        emit('error', { op: 'playLater', message: String((e && e.message) || e) });
      });
  }

  saint.play = function () {
    if (!saint.music) return;
    var t0 = Date.now();
    var m = saint.music;
    var mine = pending;
    pendingQueue
      .then(function () {
        log('play: START (queue len ' + ((m.queue && m.queue.length) || 0) + ')');
        return m.play();
      })
      .then(function () {
        log('play: RESOLVED in ' + (Date.now() - t0) + 'ms');
        appendRest(m, mine);
      })
      .catch(function (e) {
        log('play: FAILED after ' + (Date.now() - t0) + 'ms ' + String((e && e.message) || e));
        emit('error', { op: 'play', message: String((e && e.message) || e) });
      });
  };

  saint.prewarm = function (id) {
    if (!saint.music || saint.__prewarmed) return;
    saint.__prewarmed = true;

    var m = saint.music;
    var restore = m.volume;
    var t0 = Date.now();
    saint.__prewarming = true;
    try { m.volume = 0; } catch (e) {}

    function abortIfClicked() {
      if (!saint.__prewarming) throw new Error('cancelled by a real play');
    }

    m.setQueue({ songs: [id], startWith: 0 })
      .then(function () { abortIfClicked(); return m.play(); })
      .then(function () { abortIfClicked(); return m.stop(); })
      .then(function () { log('prewarm: done in ' + (Date.now() - t0) + 'ms'); })
      .catch(function (e) { log('prewarm: skipped ' + String((e && e.message) || e)); })
      .then(function () {
        saint.__prewarming = false;
        try { m.volume = restore; } catch (e) {}
      });
  };

  saint.pause = function () {
    if (!saint.music) return;
    try { saint.music.pause(); } catch (e) {
      emit('error', { op: 'pause', message: String(e && e.message || e) });
    }
  };

  saint.seek = function (seconds) {
    if (!saint.music) return;
    saint.music.seekToTime(seconds).catch(function (e) {
      emit('error', { op: 'seek', message: String(e && e.message || e) });
    });
  };

  saint.loadRecent = function () {
    if (!saint.music) return;
    saint.music.api
      .music('/v1/me/recent/played/tracks', { limit: 15 })
      .then(function (r) {
        var items = (r && r.data && r.data.data) || [];
        var tracks = [];
        for (var i = 0; i < items.length; i++) {
          var it = items[i];
          if (it.type !== 'songs') continue;
          var a = it.attributes || {};
          tracks.push({
            id: String(it.id),
            title: a.name || '',
            artist: a.artistName || '',
            album: a.albumName || '',
            duration_ms: a.durationInMillis || 0
          });
        }
        emit('recentTracks', { tracks: tracks });
      })
      .catch(function (e) {
        emit('error', { op: 'loadRecent', message: String((e && e.message) || e) });
      });
  };

  saint.skipNext = function () {
    if (!saint.music) return;
    Promise.resolve(saint.music.skipToNextItem()).catch(function (e) {
      emit('error', { op: 'skipNext', message: String((e && e.message) || e) });
    });
  };

  saint.skipPrevious = function () {
    if (!saint.music) return;
    Promise.resolve(saint.music.skipToPreviousItem()).catch(function (e) {
      emit('error', { op: 'skipPrevious', message: String((e && e.message) || e) });
    });
  };

  saint.setShuffle = function (on) {
    if (!saint.music) return;
    try { saint.music.shuffleMode = on ? 1 : 0; } catch (e) {
      emit('error', { op: 'setShuffle', message: String((e && e.message) || e) });
    }
  };

  saint.setRepeat = function (mode) {
    if (!saint.music) return;
    try { saint.music.repeatMode = mode; } catch (e) {
      emit('error', { op: 'setRepeat', message: String((e && e.message) || e) });
    }
  };

  saint.setVolume = function (unit) {
    if (!saint.music) return;
    try { saint.music.volume = unit; } catch (e) {
      emit('error', { op: 'setVolume', message: String(e && e.message || e) });
    }
  };
})();
