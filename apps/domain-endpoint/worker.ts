export default {
  fetch(request: Request) {
    const url = new URL(request.url);
    if (url.protocol === 'http:') {
      url.protocol = 'https:';
      return Response.redirect(url.href, 308);
    }
    return new Response(null, {
      status: 404,
      headers: { 'Cache-Control': 'no-store' },
    });
  },
};
