module ApplicationHelper
  def html_class
    classes = []
    classes << 'view--logged-in' if user_signed_in?
    classes << "view--#{controller_name}-#{action_name}"
    classes.join(' ')
  end

  def render_turbo_stream_flash_messages
    turbo_stream.prepend 'flash', partial: 'layouts/flash'
  end
end
