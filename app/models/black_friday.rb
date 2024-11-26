class BlackFriday
  START_DATE = Date.new(Date.current.year, 11, 25)
  END_DATE = Date.new(Date.current.year, 12, 1)
  DISCOUNT_PERCENT = 0.3

  def self.week?
    Date.current.between?(START_DATE, END_DATE)
  end

  def self.end_time
    END_DATE.end_of_day
  end
end
