require 'administrate/field/base'

class SubscriptionField < Administrate::Field::Base
  def to_s
    return if data.free?

    if data.ends_at.nil?
      "#{data.name} (lifetime)"
    else
      days = (data.ends_at.to_date - Date.today).to_i
      "#{data.name} (#{days} days left)"
    end
  end
end
