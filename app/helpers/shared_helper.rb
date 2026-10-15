module SharedHelper
  # These helper methods are shared between views and controllers

  def turbo_stream_prepend_flash
    turbo_stream.prepend('flash', partial: 'layouts/flash')
  end

  def turbo_stream_page_refresh
    turbo_stream.refresh(request_id: nil)
  end

  def turbo_stream_redirect(path)
    turbo_stream.action(:redirect, path)
  end

  # A full page load, not a Turbo visit: a visit carries every data-turbo-permanent node across as it
  # stands, so anything those nodes rendered for the old page (the menu's locale-prefixed links) stays.
  def turbo_stream_reload(path)
    turbo_stream.action(:reload, path)
  end
end
