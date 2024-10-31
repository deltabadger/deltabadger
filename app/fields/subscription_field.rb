require 'administrate/field/base'

class SubscriptionField < Administrate::Field::Base
  def to_s
    if data.unlimited?
      days = (data.end_time.to_date - Date.today).to_i
      "#{localized_plan_name(data.name)} (#{days} days left)"
    else
      "#{localized_plan_name(data.name)} (#{data.credits.round(2)})"
    end
  end
end
