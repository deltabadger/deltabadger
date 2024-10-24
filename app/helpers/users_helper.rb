module UsersHelper
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
