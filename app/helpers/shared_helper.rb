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
end
