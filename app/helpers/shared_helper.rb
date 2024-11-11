module SharedHelper
  # These helper methods are shared between views and controllers

  def turbo_stream_prepend_flash
    turbo_stream.prepend 'flash', partial: 'layouts/flash'
  end
end
