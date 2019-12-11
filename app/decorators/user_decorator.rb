class UserDecorator < SimpleDelegator
  def plan_days_left
    (subscription.end_time.to_date - Date.today).to_i
  end
end
