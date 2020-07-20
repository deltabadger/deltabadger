require 'administrate/field/base'

class SubscriptionField < Administrate::Field::Base
  def to_s
    if data.unlimited?
      days = (data.end_time.to_date - Date.today).to_i
      "#{data.display_name} (#{days} days left)"
    else
      "#{data.display_name} (#{data.credits.round(2)})"
    end
  end
end
