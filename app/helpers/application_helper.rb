module ApplicationHelper
  def main_html_classes
    classes = []
    classes << 'view--logged-in' if user_signed_in?
    classes << "view--#{controller_name}-#{action_name}"
    classes.join(' ')
  end

  def render_turbo_stream_flash_messages
    turbo_stream.prepend 'flash', partial: 'layouts/flash'
  end

  def show_limit_reached_navbar?
    current_user.limit_reached? &&
      !action?('upgrade', 'index') &&
      !action?('affiliates', 'new') &&
      !action?('affiliates', 'create')
  end

  private

  def action?(controller, action)
    params[:controller] == controller && params[:action] == action
  end
end
