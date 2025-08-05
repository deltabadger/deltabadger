class Payments::Wire < Payment
  validates :first_name, :last_name, presence: true
end
