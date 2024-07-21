class UserDecorator < ActiveRecordDecorator
  def initialize(user:, context:)
    @context = context
    super(user)
  end

  delegate :email, :name, :id, to: :__getobj__

  def username
    __getobj__.email.split('@').first # Use email as the username
  end

  def plan_days_left
    (subscription.end_time.to_date - Date.today).to_i
  end

  def show_limit_reached_navbar?
    limit_reached? &&
      !action?('upgrade', 'index') &&
      !action?('affiliates', 'new') &&
      !action?('affiliates', 'create')
  end

  private

  def action?(controller, action)
    context.params[:controller] == controller &&
      context.params[:action] == action
  end

  attr_reader :context
end
