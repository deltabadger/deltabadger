class BaseRepository
  def save(object)
    object.save!
  end
end
