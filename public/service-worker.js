self.addEventListener('install', function(event) {
  event.waitUntil(
    caches.open('deltabadger-cache').then(function(cache) {
      return cache.addAll([
        '/',
        // 'favicon/web-app-manifest-192x192.png',
        // 'favicon/web-app-manifest-512x512.png',
        // 'favicon/site.webmanifest',
        // List of other assets to cache
      ]);
    })
  );
});

self.addEventListener('fetch', function(event) {
  event.respondWith(
    caches.match(event.request).then(function(response) {
      if (response) {
        return response;
      }
      return fetch(event.request).catch(function() {
        // Silently fail for requests that can't be fetched
        return new Response('', { status: 408, statusText: 'Request failed' });
      });
    })
  );
});