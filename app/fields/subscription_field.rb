require 'administrate/field/base'

class SubscriptionField < Administrate::Field::Base
  def to_s
    if data.unlimited?
      days = (data.end_time.to_date - Date.today).to_i
      "#{data.name.capitalize} (#{days} days left)"
    else
      "#{data.name.capitalize} (#{data.credits.round(2)})"
    end
  end
end
