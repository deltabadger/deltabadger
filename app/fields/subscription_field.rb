require 'administrate/field/base'

class SubscriptionField < Administrate::Field::Base
  def to_s
    return unless data.unlimited?

    if data.end_time.nil?
      "#{data.name} (lifetime)"
    else
      days = (data.end_time.to_date - Date.today).to_i
      "#{data.name} (#{days} days left)"
    end
  end
end
