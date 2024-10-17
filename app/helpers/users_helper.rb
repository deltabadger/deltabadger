module UsersHelper
  def subscribed_plan_days_left
    @subscribed_plan_days_left ||= (current_user.subscription.end_time.to_date - Date.today).to_i
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
